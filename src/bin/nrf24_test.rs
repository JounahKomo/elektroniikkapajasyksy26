#![no_std]
#![no_main]

use defmt::{error, info, warn};
use embassy_executor::Spawner;
use embassy_stm32::gpio::{Level, Output, Speed};
use embassy_stm32::mode::Blocking;
use embassy_stm32::spi::{Config, Spi};
use embassy_stm32::time::Hertz;
use embassy_time::{Duration, Timer};
use {defmt_rtt as _, panic_probe as _};

// NRF24L01 SPI commands
const CMD_R_REGISTER: u8 = 0x00;
const CMD_W_REGISTER: u8 = 0x20;

// NRF24L01 Register addresses
const REG_CONFIG: u8 = 0x00;
const REG_SETUP_AW: u8 = 0x03;
const REG_RF_CH: u8 = 0x05;
const REG_RF_SETUP: u8 = 0x06;
const REG_STATUS: u8 = 0x07;
const REG_TX_ADDR: u8 = 0x10;
const REG_FIFO_STATUS: u8 = 0x17;

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_stm32::init(Default::default());

    // Onboard LED LD3 (PB3) as heartbeat indicator
    let mut led = Output::new(p.PB3, Level::Low, Speed::Low);

    // CE (PA8, D9) - Chip Enable (active high for RX/TX operations, low for standby/config)
    let mut ce = Output::new(p.PA8, Level::Low, Speed::Low);
    ce.set_low();

    // CSN (PA11, D10) - Chip Select Not (idle HIGH)
    let mut csn = Output::new(p.PA11, Level::High, Speed::High);
    csn.set_high();

    // Give RTT / probe-rs a moment to attach before logging
    Timer::after(Duration::from_millis(500)).await;
    info!("=== NRF24L01 Test Starting ===");

    // SPI1 configuration: Mode 0 (CPOL = 0, CPHA = 0), 1 MHz
    let mut spi_config = Config::default();
    spi_config.frequency = Hertz(1_000_000);

    // SCK = PA5 (A4), MOSI = PA7 (A6), MISO = PA6 (A5)
    let mut spi = Spi::new_blocking(p.SPI1, p.PA5, p.PA7, p.PA6, spi_config);

    let mut loop_count: u32 = 0;

    loop {
        loop_count += 1;
        led.set_high();
        info!("--- Loop #{} ---", loop_count);

        let config_val = read_register(&mut spi, &mut csn, REG_CONFIG);
        let status_val = read_register(&mut spi, &mut csn, REG_STATUS);
        let rf_ch_val = read_register(&mut spi, &mut csn, REG_RF_CH);
        let rf_setup_val = read_register(&mut spi, &mut csn, REG_RF_SETUP);
        let setup_aw_val = read_register(&mut spi, &mut csn, REG_SETUP_AW);
        let fifo_status_val = read_register(&mut spi, &mut csn, REG_FIFO_STATUS);

        info!(
            "Regs -> CONFIG: 0x{:02x}, STATUS: 0x{:02x}, RF_CH: 0x{:02x}, RF_SETUP: 0x{:02x}, SETUP_AW: 0x{:02x}, FIFO_STATUS: 0x{:02x}",
            config_val, status_val, rf_ch_val, rf_setup_val, setup_aw_val, fifo_status_val
        );

        // Read TX_ADDR (5 bytes, default is typically 0xE7, 0xE7, 0xE7, 0xE7, 0xE7)
        let mut tx_addr = [0u8; 5];
        read_register_multi(&mut spi, &mut csn, REG_TX_ADDR, &mut tx_addr);
        info!("TX_ADDR: {=[u8]:02x}", tx_addr);

        // Write test value to RF_CH (0x2A), read back, then restore 0x02
        write_register(&mut spi, &mut csn, REG_RF_CH, 0x2A);
        let ch_readback = read_register(&mut spi, &mut csn, REG_RF_CH);
        write_register(&mut spi, &mut csn, REG_RF_CH, 0x02);

        if config_val == 0x00 {
            error!("MISO reading all 0x00! Check VCC/GND power and pin alignment.");
        } else if config_val == 0xFF {
            warn!("MISO reading all 0xFF (floating/pulled high). Chip not responding.");
        } else if ch_readback == 0x2A {
            info!("SUCCESS: NRF24L01 SPI communication verified! (RF_CH readback: 0x2A)");
        } else {
            error!(
                "SPI write verification failed: expected 0x2A, read back 0x{:02x}",
                ch_readback
            );
        }

        led.set_low();
        Timer::after(Duration::from_millis(1500)).await;
    }
}

fn read_register<'d>(spi: &mut Spi<'d, Blocking>, csn: &mut Output<'d>, reg: u8) -> u8 {
    let mut buf = [CMD_R_REGISTER | (reg & 0x1F), 0xFF];
    csn.set_low();
    let res = spi.blocking_transfer_in_place(&mut buf);
    csn.set_high();
    if let Err(_e) = res {
        error!("SPI transfer error in read_register");
    }
    buf[1]
}

fn write_register<'d>(spi: &mut Spi<'d, Blocking>, csn: &mut Output<'d>, reg: u8, val: u8) {
    let mut buf = [CMD_W_REGISTER | (reg & 0x1F), val];
    csn.set_low();
    let res = spi.blocking_transfer_in_place(&mut buf);
    csn.set_high();
    if let Err(_e) = res {
        error!("SPI transfer error in write_register");
    }
}

fn read_register_multi<'d>(
    spi: &mut Spi<'d, Blocking>,
    csn: &mut Output<'d>,
    reg: u8,
    out: &mut [u8],
) {
    csn.set_low();
    let mut cmd = [CMD_R_REGISTER | (reg & 0x1F)];
    let _ = spi.blocking_transfer_in_place(&mut cmd);
    for byte in out.iter_mut() {
        let mut b = [0xFF];
        let _ = spi.blocking_transfer_in_place(&mut b);
        *byte = b[0];
    }
    csn.set_high();
}
