use defmt::{error, info, warn};
use embassy_stm32::gpio::Output;
use embassy_stm32::mode::Blocking;
use embassy_stm32::spi::Spi;
use embassy_time::{Duration, Timer};

// NRF24L01 SPI commands
pub const CMD_R_REGISTER: u8 = 0x00;
pub const CMD_W_REGISTER: u8 = 0x20;
pub const CMD_R_RX_PAYLOAD: u8 = 0x61;
pub const CMD_W_TX_PAYLOAD: u8 = 0xA0;
pub const CMD_FLUSH_TX: u8 = 0xE1;
pub const CMD_FLUSH_RX: u8 = 0xE2;
pub const CMD_NOP: u8 = 0xFF;

// NRF24L01 Register addresses
pub const REG_CONFIG: u8 = 0x00;
pub const REG_EN_AA: u8 = 0x01;
pub const REG_EN_RXADDR: u8 = 0x02;
pub const REG_SETUP_AW: u8 = 0x03;
pub const REG_SETUP_RETR: u8 = 0x04;
pub const REG_RF_CH: u8 = 0x05;
pub const REG_RF_SETUP: u8 = 0x06;
pub const REG_STATUS: u8 = 0x07;
pub const REG_RX_ADDR_P0: u8 = 0x0A;
pub const REG_TX_ADDR: u8 = 0x10;
pub const REG_RX_PW_P0: u8 = 0x11;
pub const REG_FIFO_STATUS: u8 = 0x17;

pub const PAYLOAD_SIZE: usize = 32;

// Default 5-byte pipe address and RF channel (2476 MHz avoids WiFi congestion)
pub const DEFAULT_ADDR: [u8; 5] = [0xE7, 0xE7, 0xE7, 0xE7, 0xE7];
pub const DEFAULT_CHANNEL: u8 = 76;

#[derive(Debug, PartialEq, Eq, defmt::Format)]
pub enum TxError {
    MaxRetries,
    Timeout,
}

pub struct Nrf24<'d> {
    spi: Spi<'d, Blocking>,
    ce: Output<'d>,
    csn: Output<'d>,
}

impl<'d> Nrf24<'d> {
    pub fn new(spi: Spi<'d, Blocking>, ce: Output<'d>, csn: Output<'d>) -> Self {
        let mut dev = Self { spi, ce, csn };
        dev.ce.set_low();
        dev.csn.set_high();
        dev
    }

    pub fn read_reg(&mut self, reg: u8) -> u8 {
        let mut buf = [CMD_R_REGISTER | (reg & 0x1F), 0xFF];
        self.csn.set_low();
        let res = self.spi.blocking_transfer_in_place(&mut buf);
        self.csn.set_high();
        if let Err(_e) = res {
            error!("SPI transfer error in read_reg");
        }
        buf[1]
    }

    pub fn write_reg(&mut self, reg: u8, val: u8) {
        let mut buf = [CMD_W_REGISTER | (reg & 0x1F), val];
        self.csn.set_low();
        let res = self.spi.blocking_transfer_in_place(&mut buf);
        self.csn.set_high();
        if let Err(_e) = res {
            error!("SPI transfer error in write_reg");
        }
    }

    pub fn read_reg_multi(&mut self, reg: u8, out: &mut [u8]) {
        self.csn.set_low();
        let mut cmd = [CMD_R_REGISTER | (reg & 0x1F)];
        let _ = self.spi.blocking_transfer_in_place(&mut cmd);
        for byte in out.iter_mut() {
            let mut b = [0xFF];
            let _ = self.spi.blocking_transfer_in_place(&mut b);
            *byte = b[0];
        }
        self.csn.set_high();
    }

    pub fn write_reg_multi(&mut self, reg: u8, data: &[u8]) {
        self.csn.set_low();
        let mut cmd = [CMD_W_REGISTER | (reg & 0x1F)];
        let _ = self.spi.blocking_transfer_in_place(&mut cmd);
        for &byte in data.iter() {
            let mut b = [byte];
            let _ = self.spi.blocking_transfer_in_place(&mut b);
        }
        self.csn.set_high();
    }

    pub fn send_command(&mut self, cmd: u8) {
        self.csn.set_low();
        let mut buf = [cmd];
        let _ = self.spi.blocking_transfer_in_place(&mut buf);
        self.csn.set_high();
    }

    /// Initializes basic NRF24L01 configuration (channel, data rate, payload width).
    /// Returns true if communication with the chip is verified.
    pub async fn init(&mut self) -> bool {
        self.ce.set_low();
        self.csn.set_high();
        Timer::after(Duration::from_millis(10)).await;

        // 5-byte address width
        self.write_reg(REG_SETUP_AW, 0x03);

        // Auto Retransmit: 750 µs delay (0x20), 15 retries (0x0F)
        self.write_reg(REG_SETUP_RETR, 0x2F);

        // RF Channel 76 (2476 MHz)
        self.write_reg(REG_RF_CH, DEFAULT_CHANNEL);

        // 1 Mbps, 0 dBm output power (highest power)
        self.write_reg(REG_RF_SETUP, 0x06);

        // Enable Auto-Ack on Pipe 0
        self.write_reg(REG_EN_AA, 0x01);

        // Enable RX Data Pipe 0
        self.write_reg(REG_EN_RXADDR, 0x01);

        // Set fixed payload size (32 bytes) for Pipe 0
        self.write_reg(REG_RX_PW_P0, PAYLOAD_SIZE as u8);

        // Set TX and RX pipe 0 addresses (must match for Auto-Ack)
        self.write_reg_multi(REG_TX_ADDR, &DEFAULT_ADDR);
        self.write_reg_multi(REG_RX_ADDR_P0, &DEFAULT_ADDR);

        // Clear any pending interrupt flags
        self.write_reg(REG_STATUS, 0x70);

        // Flush FIFOs
        self.send_command(CMD_FLUSH_TX);
        self.send_command(CMD_FLUSH_RX);

        // Verify chip response
        let ch = self.read_reg(REG_RF_CH);
        let config = self.read_reg(REG_CONFIG);
        if ch != DEFAULT_CHANNEL || config == 0x00 || config == 0xFF {
            warn!(
                "NRF24L01 communication check failed! RF_CH read: {}, CONFIG: 0x{:02x}",
                ch, config
            );
            return false;
        }

        info!("NRF24L01 initialized successfully on 2476 MHz (CH: {})", ch);
        true
    }

    /// Configures the radio as a Transmitter (PTX).
    pub async fn config_tx(&mut self) {
        self.ce.set_low();
        // Power up, TX mode (PRIM_RX=0), 2-byte CRC (EN_CRC=1, CRCO=1) -> 0x0E
        self.write_reg(REG_CONFIG, 0x0E);
        self.write_reg(REG_STATUS, 0x70);
        self.send_command(CMD_FLUSH_TX);
        // Allow crystal oscillator to settle
        Timer::after(Duration::from_millis(2)).await;
    }

    /// Configures the radio as a Receiver (PRX) and begins listening.
    pub async fn config_rx(&mut self) {
        self.ce.set_low();
        // Power up, RX mode (PRIM_RX=1), 2-byte CRC (EN_CRC=1, CRCO=1) -> 0x0F
        self.write_reg(REG_CONFIG, 0x0F);
        self.write_reg(REG_STATUS, 0x70);
        self.send_command(CMD_FLUSH_RX);
        // CE high starts active receiver listening
        self.ce.set_high();
        // Allow receiver PLL to settle
        Timer::after(Duration::from_millis(2)).await;
    }

    /// Enables or disables Auto-Acknowledgement (Enhanced ShockBurst).
    pub fn set_auto_ack(&mut self, enable: bool) {
        if enable {
            self.write_reg(REG_EN_AA, 0x01);
            self.write_reg(REG_SETUP_RETR, 0x2F);
        } else {
            self.write_reg(REG_EN_AA, 0x00);
            self.write_reg(REG_SETUP_RETR, 0x00);
        }
    }

    /// Transmits a 32-byte payload. Blocks until transmission is acknowledged
    /// or max retries are exceeded.
    pub async fn send_packet(&mut self, payload: &[u8; PAYLOAD_SIZE]) -> Result<(), TxError> {
        self.ce.set_low();
        self.write_reg(REG_STATUS, 0x70);
        self.send_command(CMD_FLUSH_TX);

        // Load payload into TX FIFO
        self.csn.set_low();
        let _ = self.spi.blocking_transfer_in_place(&mut [CMD_W_TX_PAYLOAD]);
        let mut buf = *payload;
        let _ = self.spi.blocking_transfer_in_place(&mut buf);
        self.csn.set_high();

        // Pulse CE high for at least 15 µs to start RF transmission
        self.ce.set_high();
        Timer::after(Duration::from_micros(20)).await;
        self.ce.set_low();

        // Poll for completion (up to ~50 ms timeout)
        for _ in 0..500 {
            let status = self.read_reg(REG_STATUS);
            if (status & 0x20) != 0 {
                // TX_DS: Successfully transmitted & acknowledged!
                self.write_reg(REG_STATUS, 0x20);
                return Ok(());
            }
            if (status & 0x10) != 0 {
                // MAX_RT: Retries exhausted, no ACK received
                self.write_reg(REG_STATUS, 0x10);
                self.send_command(CMD_FLUSH_TX);
                return Err(TxError::MaxRetries);
            }
            Timer::after(Duration::from_micros(100)).await;
        }

        self.send_command(CMD_FLUSH_TX);
        Err(TxError::Timeout)
    }

    /// Non-blocking transmit: loads the payload, pulses CE and returns immediately
    /// (roughly 100 us). The caller must later call [`Nrf24::finish_send`].
    /// Intended for real-time streaming where the CPU cannot wait for the ACK.
    pub fn start_send(&mut self, payload: &[u8; PAYLOAD_SIZE]) {
        self.ce.set_low();
        self.write_reg(REG_STATUS, 0x70);
        self.send_command(CMD_FLUSH_TX);

        // Single SPI transaction: command + 32 payload bytes
        let mut buf = [0u8; PAYLOAD_SIZE + 1];
        buf[0] = CMD_W_TX_PAYLOAD;
        buf[1..].copy_from_slice(payload);
        self.csn.set_low();
        let _ = self.spi.blocking_transfer_in_place(&mut buf);
        self.csn.set_high();

        // CE high for >= 15 us (1600 cycles = 20 us at 80 MHz)
        self.ce.set_high();
        cortex_m::asm::delay(1600);
        self.ce.set_low();
    }

    /// Collects the result of [`Nrf24::start_send`]. Returns `Some(true)` if the packet was
    /// acknowledged, `Some(false)` if retries ran out, and `None` if it is still in flight
    /// (in which case the pending packet is discarded).
    pub fn finish_send(&mut self) -> Option<bool> {
        let status = self.read_reg(REG_STATUS);
        let result = if (status & 0x20) != 0 {
            Some(true)
        } else if (status & 0x10) != 0 {
            Some(false)
        } else {
            None
        };
        if result != Some(true) {
            self.send_command(CMD_FLUSH_TX);
        }
        self.write_reg(REG_STATUS, 0x70);
        result
    }

    /// Checks if a payload is available in the RX FIFO.
    pub fn is_rx_data_ready(&mut self) -> bool {
        let status = self.read_reg(REG_STATUS);
        // Bit 6: RX_DR (data ready), or bits 3:1 (RX_P_NO != 0b111)
        (status & 0x40) != 0 || ((status >> 1) & 0x07) <= 0x05
    }

    /// Reads a 32-byte payload from the RX FIFO if available.
    /// Returns true if a packet was read.
    pub fn read_packet(&mut self, out: &mut [u8; PAYLOAD_SIZE]) -> bool {
        if !self.is_rx_data_ready() {
            return false;
        }

        // Single SPI transaction (command + 32 payload bytes) to keep CSN low as briefly as possible
        let mut buf = [0xFFu8; PAYLOAD_SIZE + 1];
        buf[0] = CMD_R_RX_PAYLOAD;
        self.csn.set_low();
        let _ = self.spi.blocking_transfer_in_place(&mut buf);
        self.csn.set_high();
        out.copy_from_slice(&buf[1..]);

        // Clear RX_DR flag in STATUS
        self.write_reg(REG_STATUS, 0x40);
        true
    }
}
