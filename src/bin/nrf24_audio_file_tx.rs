#![no_std]
#![no_main]

//! NRF24L01 audio file transmitter test: streams `audio/audio_testi.raw` over the radio.
//! Compatible with `nrf24_audio_rx`.
//!
//! The file is raw 8-bit unsigned PCM (silence = 128), mono, 8,000 Hz. It is sent as
//! 32-byte payloads of 32 samples (4 ms of audio each) on channel 76 / address
//! E7:E7:E7:E7:E7, one packet every 4 ms so the receiver plays it in real time. The
//! clip repeats forever with a pause in between.
//!
//! Radio (SPI1): SCK = PA5, MISO = PA6, MOSI = PA7, CSN = PA11, CE = PB0.

use defmt::{error, info};
use embassy_executor::Spawner;
use embassy_stm32::gpio::{Level, Output, Speed};
use embassy_stm32::spi::{Config, Spi};
use embassy_stm32::time::Hertz;
use embassy_time::{Duration, Ticker, Timer};
use nucleo_l432kc_app::nrf24::{Nrf24, PAYLOAD_SIZE};
use {defmt_rtt as _, panic_probe as _};

/// Audio clip: 8-bit unsigned PCM, 8 kHz, mono.
static AUDIO: &[u8] = include_bytes!("../../audio/audio_testi.raw");

/// Time between packets: 32 samples @ 8 kHz = 4 ms.
const PACKET_PERIOD: Duration = Duration::from_millis(4);
/// Pause between repetitions of the clip.
const REPEAT_PAUSE: Duration = Duration::from_secs(2);

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_stm32::init(Default::default());

    let ce = Output::new(p.PB0, Level::Low, Speed::Low);
    let csn = Output::new(p.PA11, Level::High, Speed::High);

    Timer::after(Duration::from_millis(500)).await;
    info!("=======================================================");
    info!("=== NRF24L01 Audio File Transmitter (audio_testi)   ===");
    info!("=======================================================");
    info!(
        "{} samples = {} packets, {} ms of audio.",
        AUDIO.len(),
        AUDIO.len().div_ceil(PAYLOAD_SIZE),
        AUDIO.len() / 8
    );

    let mut spi_config = Config::default();
    spi_config.frequency = Hertz(4_000_000);
    let spi = Spi::new_blocking(p.SPI1, p.PA5, p.PA7, p.PA6, spi_config);
    let mut radio = Nrf24::new(spi, ce, csn);

    if !radio.init().await {
        error!("Failed to initialize NRF24L01! Check wiring, 3.3V power, and GND.");
        loop {
            Timer::after(Duration::from_secs(1)).await;
        }
    }
    radio.config_tx().await;
    info!("Radio configured as transmitter on channel 76.");

    loop {
        info!("Sending clip...");
        let mut ticker = Ticker::every(PACKET_PERIOD);
        let mut sent: u32 = 0;
        let mut lost: u32 = 0;

        for chunk in AUDIO.chunks(PAYLOAD_SIZE) {
            // Pad the last packet with silence
            let mut payload = [128u8; PAYLOAD_SIZE];
            payload[..chunk.len()].copy_from_slice(chunk);

            match radio.send_packet(&payload).await {
                Ok(()) => sent += 1,
                Err(_) => lost += 1,
            }
            ticker.next().await;
        }

        info!("Clip done: {} packets acked, {} lost/no-ack.", sent, lost);
        Timer::after(REPEAT_PAUSE).await;
    }
}
