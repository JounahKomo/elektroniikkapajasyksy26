#![no_std]
#![no_main]

//! NRF24L01 audio receiver test: listens for audio packets and plays them on the speaker.
//!
//! Packet format (matches the `audio/*.raw` library): every 32-byte payload carries
//! 32 samples of 8-bit unsigned PCM (silence = 128) at 8,000 Hz, i.e. 4 ms of audio.
//! Any transmitter that sends such packets on channel 76 / address E7:E7:E7:E7:E7 works.
//!
//! Radio (SPI1): SCK = PA5, MISO = PA6, MOSI = PA7, CSN = PA11, CE = PB0.
//! Speaker (I2S): BCLK = PA3, LRC = PA4, DIN = PA10.
//!
//! The I2S clocks are bit-banged by the CPU and must run *perfectly continuously*: any pause
//! (such as a blocking SPI transfer to the radio) makes the MAX98357A drop out and stay silent.
//! So the radio is read with a register-level SPI state machine (`RadioPoller`) that is
//! stepped from inside the I2S bit loop, one tiny step every fourth BCLK bit. A jitter buffer absorbs
//! packet timing variation: playback starts once `PREBUFFER` samples have arrived and
//! re-buffers after an underrun.

use defmt::{error, info};
use embassy_executor::Spawner;
use embassy_stm32::gpio::{Level, Output, Speed};
use embassy_stm32::spi::{Config, Spi};
use embassy_stm32::time::Hertz;
use nucleo_l432kc_app::max98357a::{config_80mhz, Max98357a};
use nucleo_l432kc_app::nrf24::{Nrf24, PAYLOAD_SIZE};
use {defmt_rtt as _, panic_probe as _};

/// Jitter buffer capacity in samples (must be a power of two): 2048 = 256 ms.
const BUF_SIZE: usize = 2048;
/// Samples to collect before playback (re)starts: 512 = 64 ms.
const PREBUFFER: usize = 512;

/// Playback volume shift (0 = unchanged, 1 = 2x, ...). Samples are clipped.
const VOLUME_SHIFT: u32 = 0;

/// Simple FIFO of 8-bit samples.
struct Jitter {
    data: [u8; BUF_SIZE],
    head: usize,
    count: usize,
}

impl Jitter {
    const fn new() -> Self {
        Self {
            data: [128; BUF_SIZE],
            head: 0,
            count: 0,
        }
    }

    /// Append a packet. Returns false (and drops it) if it does not fit.
    fn push(&mut self, samples: &[u8]) -> bool {
        if self.count + samples.len() > BUF_SIZE {
            return false;
        }
        for &s in samples {
            self.data[(self.head + self.count) & (BUF_SIZE - 1)] = s;
            self.count += 1;
        }
        true
    }

    fn pop(&mut self) -> u8 {
        let s = self.data[self.head];
        self.head = (self.head + 1) & (BUF_SIZE - 1);
        self.count -= 1;
        s
    }
}

const SPI1_BASE: usize = 0x4001_3000;
const SPI_CR2: *mut u32 = (SPI1_BASE + 0x04) as *mut u32;
const SPI_SR: *const u32 = (SPI1_BASE + 0x08) as *const u32;
const SPI_DR8: *mut u8 = (SPI1_BASE + 0x0C) as *mut u8;
const SR_RXNE: u32 = 1 << 0;
const CR2_FRXTH: u32 = 1 << 12;
const GPIOA_BSRR: *mut u32 = 0x4800_0018 as *mut u32;
const CSN_LOW: u32 = 1 << (11 + 16); // PA11
const CSN_HIGH: u32 = 1 << 11;
const CMD_NOP: u8 = 0xFF;
const CMD_R_RX_PAYLOAD: u8 = 0x61;

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    /// Nothing running: start a STATUS read next step.
    Idle,
    /// Reading the STATUS register (1 byte).
    Status,
    /// Pending start of the payload read (keeps CSN high for a moment in between).
    ReadStart,
    /// Reading a payload from the RX FIFO (1 command + 32 bytes).
    Read,
    /// A packet is waiting in `buf[1..]`; paused until the main loop has consumed it.
    Hold,
}

/// Non-blocking SPI1 master driving the NRF24 receive path. [`RadioPoller::step`] moves one
/// byte at a time using the SPI hardware flags and never waits, so it can be called from the
/// I2S bit loop. SPI1 must already be configured (by `Nrf24`/embassy) before use.
struct RadioPoller {
    stage: Stage,
    buf: [u8; PAYLOAD_SIZE + 1],
    len: usize,
    sent: usize,
    recvd: usize,
}

impl RadioPoller {
    fn new() -> Self {
        // Raise the RXNE threshold to 8 bits so each received byte sets RXNE
        unsafe { core::ptr::write_volatile(SPI_CR2, core::ptr::read_volatile(SPI_CR2) | CR2_FRXTH) };
        Self {
            stage: Stage::Idle,
            buf: [0; PAYLOAD_SIZE + 1],
            len: 0,
            sent: 0,
            recvd: 0,
        }
    }

    #[inline(always)]
    fn begin(&mut self, stage: Stage, len: usize) {
        self.stage = stage;
        self.len = len;
        self.sent = 0;
        self.recvd = 0;
        unsafe { core::ptr::write_volatile(GPIOA_BSRR, CSN_LOW) };
    }

    /// Advance the SPI job by at most one byte. Takes a few dozen cycles at most.
    #[inline(always)]
    fn step(&mut self) {
        match self.stage {
            Stage::Hold => {}
            Stage::Idle => {
                self.buf[0] = CMD_NOP;
                self.begin(Stage::Status, 1);
            }
            Stage::Status | Stage::Read => {
                let sr = unsafe { core::ptr::read_volatile(SPI_SR) };
                // Collect the byte that has finished shifting (if any) ...
                if self.sent > self.recvd && sr & SR_RXNE != 0 {
                    let b = unsafe { core::ptr::read_volatile(SPI_DR8) };
                    unsafe { *self.buf.get_unchecked_mut(self.recvd) = b };
                    self.recvd += 1;
                }
                // ... and start the next one once the previous is fully received.
                if self.sent == self.recvd {
                    if self.recvd < self.len {
                        let b = unsafe { *self.buf.get_unchecked(self.sent) };
                        unsafe { core::ptr::write_volatile(SPI_DR8, b) };
                        self.sent += 1;
                    } else {
                        // Transfer complete
                        unsafe { core::ptr::write_volatile(GPIOA_BSRR, CSN_HIGH) };
                        if self.stage == Stage::Status {
                            // RX_P_NO (STATUS bits 3:1) < 6 means the RX FIFO holds a payload
                            if ((self.buf[0] >> 1) & 0x07) <= 5 {
                                self.buf[0] = CMD_R_RX_PAYLOAD;
                                self.stage = Stage::ReadStart;
                            } else {
                                self.stage = Stage::Idle;
                            }
                        } else {
                            self.stage = Stage::Hold;
                        }
                    }
                }
            }
            Stage::ReadStart => self.begin(Stage::Read, PAYLOAD_SIZE + 1),
        }
    }

    /// Returns the 32 received audio bytes if a packet has been read.
    fn take_packet(&mut self) -> Option<&[u8]> {
        if self.stage == Stage::Hold {
            Some(&self.buf[1..])
        } else {
            None
        }
    }

    /// Resume polling after `take_packet` data has been consumed.
    fn resume(&mut self) {
        self.stage = Stage::Idle;
    }
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // 80 MHz SYSCLK via PLL for the cycle-accurate I2S timing
    let p = embassy_stm32::init(config_80mhz());

    // Onboard LED LD3: lit while audio is being played
    let mut led = Output::new(p.PB3, Level::Low, Speed::Low);
    let ce = Output::new(p.PB0, Level::Low, Speed::Low);
    let csn = Output::new(p.PA11, Level::High, Speed::High);

    info!("=======================================================");
    info!("=== NRF24L01 Audio Receiver -> MAX98357A Speaker    ===");
    info!("=======================================================");
    info!("Expecting 32-byte packets of 8-bit unsigned PCM @ 8 kHz on channel 76.");

    let mut spi_config = Config::default();
    spi_config.frequency = Hertz(5_000_000);
    let spi = Spi::new_blocking(p.SPI1, p.PA5, p.PA7, p.PA6, spi_config);
    let mut radio = Nrf24::new(spi, ce, csn);

    if !radio.init().await {
        error!("Failed to initialize NRF24L01! Check wiring, 3.3V power, and GND.");
        loop {
            led.toggle();
            embassy_time::Timer::after_millis(100).await;
        }
    }
    radio.config_rx().await;
    info!("Radio listening on channel 76.");

    let mut audio = Max98357a::new(p.PA3, p.PA4, p.PA10);

    // Startup beep: confirms the speaker path works before any radio audio arrives
    audio.play_tone(1000, 150);
    // Drop anything received while the beep was playing
    radio.config_rx().await;

    // From here on the SPI is driven by the state machine inside the I2S bit loop.
    let mut poller = RadioPoller::new();
    let mut jitter = Jitter::new();
    let mut playing = false;

    // Statistics, logged once per second
    let mut packets: u32 = 0;
    let mut overruns: u32 = 0;
    let mut underruns: u32 = 0;
    let mut frames: u32 = 0;
    let mut peak: u16 = 0;
    let mut sample: i16 = 0;

    loop {
        // Every 8 kHz sample is sent in two consecutive 16 kHz frames.
        if frames & 1 == 0 {
            if !playing && jitter.count >= PREBUFFER {
                playing = true;
                led.set_high();
            }
            if playing && jitter.count == 0 {
                playing = false;
                underruns += 1;
                led.set_low();
            }
            sample = if playing {
                let v = ((jitter.pop() as i32) - 128) << (8 + VOLUME_SHIFT);
                v.clamp(-32768, 32767) as i16
            } else {
                0
            };
            peak = peak.max(sample.unsigned_abs());
        }

        // One I2S frame; the radio SPI advances inside the idle time of each bit.
        let _ = audio.exchange_frame_hook(sample as u16, sample as u16, || poller.step());

        if let Some(pkt) = poller.take_packet() {
            packets += 1;
            if !jitter.push(pkt) {
                overruns += 1;
            }
            poller.resume();
        }

        frames += 1;
        if frames >= 16_000 {
            info!(
                "Packets: {}, buffered: {} samples, peak: {}, underruns: {}, overruns: {}, late bits: {}",
                packets,
                jitter.count,
                peak,
                underruns,
                overruns,
                audio.late_bits()
            );
            packets = 0;
            peak = 0;
            frames = 0;
        }
    }
}
