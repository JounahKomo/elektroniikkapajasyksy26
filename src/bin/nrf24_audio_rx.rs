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
//! The I2S clocks are bit-banged by the CPU and must run continuously, so the radio is
//! polled between 1 ms chunks of audio and the SPI bus runs at 4 MHz to keep each poll short.
//! A jitter buffer absorbs packet timing variation: playback starts once `PREBUFFER`
//! samples have arrived and re-buffers after an underrun.

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
/// Samples played between radio polls: 8 samples @ 8 kHz = 1 ms.
const CHUNK: usize = 8;
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
    spi_config.frequency = Hertz(4_000_000);
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

    let mut jitter = Jitter::new();
    let mut playing = false;
    let mut payload = [0u8; PAYLOAD_SIZE];

    // Statistics, logged once per second of played/idle audio
    let mut packets: u32 = 0;
    let mut overruns: u32 = 0;
    let mut underruns: u32 = 0;
    let mut chunks: u32 = 0;

    loop {
        // 1. Drain all packets waiting in the radio FIFO (a poll with no data is a single
        //    short SPI status read).
        while radio.read_packet(&mut payload) {
            packets += 1;
            if !jitter.push(&payload) {
                overruns += 1;
            }
        }

        // 2. Start / stop playback based on how much audio is buffered
        if !playing && jitter.count >= PREBUFFER {
            playing = true;
            led.set_high();
        }
        if playing && jitter.count < CHUNK {
            playing = false;
            underruns += 1;
            led.set_low();
        }

        // 3. Play 1 ms of audio (or silence), keeping the I2S clocks running continuously.
        //    Each 8 kHz sample is sent in two 16 kHz frames.
        for _ in 0..CHUNK {
            let s = if playing {
                let v = ((jitter.pop() as i32) - 128) << (8 + VOLUME_SHIFT);
                v.clamp(-32768, 32767) as i16
            } else {
                0
            };
            let _ = audio.exchange_frame(s as u16, s as u16);
            let _ = audio.exchange_frame(s as u16, s as u16);
        }

        chunks += 1;
        if chunks >= 1000 {
            info!(
                "Packets: {}, buffered: {} samples, underruns: {}, overruns: {}, late bits: {}",
                packets,
                jitter.count,
                underruns,
                overruns,
                audio.late_bits()
            );
            packets = 0;
            chunks = 0;
        }
    }
}
