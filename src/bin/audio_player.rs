#![no_std]
#![no_main]

use defmt::info;
use embassy_executor::Spawner;
use embassy_stm32::gpio::{Level, Output, Speed};
use embassy_time::{Duration, Timer};
use nucleo_l432kc_app::max98357a::{config_80mhz, Max98357a};
use {defmt_rtt as _, panic_probe as _};

/// Audio track definition
pub struct Track {
    pub name: &'static str,
    pub sample_rate: u32,
    pub data: &'static [u8],
}

// =============================================================================
// AUDIO FILE LIBRARY (from audio/ directory)
// =============================================================================
pub const TRACK_AUDIO_TESTI: Track = Track {
    name: "Audio Testi",
    sample_rate: 8000,
    data: include_bytes!("../../audio/audio_testi.raw"),
};

pub const TRACK_CHIME: Track = Track {
    name: "Chime (Wikimedia Commons)",
    sample_rate: 8000,
    data: include_bytes!("../../audio/chime.raw"),
};

pub const TRACK_BELL: Track = Track {
    name: "Bike Bell (Wikimedia Commons)",
    sample_rate: 8000,
    data: include_bytes!("../../audio/bell.raw"),
};

pub const TRACK_ROGER_BEEP: Track = Track {
    name: "Roger Beep (Walkie-talkie)",
    sample_rate: 8000,
    data: include_bytes!("../../audio/roger_beep.raw"),
};

pub const ALL_TRACKS: &[Track] = &[TRACK_AUDIO_TESTI, TRACK_CHIME, TRACK_BELL, TRACK_ROGER_BEEP];

// =============================================================================
// PLAYBACK CONFIGURATION:
// Choose an audio file to play, or set PLAY_ALL_TRACKS = true to cycle through all!
// =============================================================================
const SELECTED_TRACK: Track = TRACK_AUDIO_TESTI;
const PLAY_ALL_TRACKS: bool = false;

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // Configure system clock to 80 MHz via PLL from 4 MHz MSI
    let p = embassy_stm32::init(config_80mhz());

    // Onboard green LED LD3 (PB3) as playback indicator
    let mut led = Output::new(p.PB3, Level::Low, Speed::Low);

    Timer::after(Duration::from_millis(500)).await;
    info!("===============================================");
    info!("=== MAX98357A I2S Audio Player (80 MHz SYSCLK) ===");
    info!("===============================================");
    info!("I2S Pinout:");
    info!("  BCLK    -> PA4  (I2S Bit Clock / SCK, Nucleo A3)");
    info!("  LRC/WS  -> PA5  (I2S Left-Right Clock / WS, Nucleo A4)");
    info!("  DIN     -> PA10 (I2S Data Out from STM32, Nucleo D2)");
    info!("Power & Config:");
    info!("  VIN     -> 5V   (Nucleo 5V, or 3.3V)");
    info!("  GND     -> GND");
    info!("  SD_MODE -> 3.3V (Active, mono L+R summing)");
    info!("  GAIN    -> Leave unconnected (9 dB standard gain)");
    info!("Passive Speaker:");
    info!("  MAX98357A '+' -> Speaker terminal 1");
    info!("  MAX98357A '-' -> Speaker terminal 2");

    // Initialize MAX98357A I2S driver
    let mut amp = Max98357a::new(p.PA4, p.PA5, p.PA10);

    let mut track_idx: usize = 0;

    loop {
        let current_track = if PLAY_ALL_TRACKS {
            let t = &ALL_TRACKS[track_idx % ALL_TRACKS.len()];
            track_idx += 1;
            t
        } else {
            &SELECTED_TRACK
        };

        let duration_ms = (current_track.data.len() as u32 * 1000) / current_track.sample_rate;
        info!(
            "Playing audio track: \"{}\" ({} bytes, ~{} ms @ {} Hz)...",
            current_track.name,
            current_track.data.len(),
            duration_ms,
            current_track.sample_rate
        );

        led.set_high();

        // Stream audio to MAX98357A I2S amplifier
        amp.play_pcm_u8(current_track.data, current_track.sample_rate);

        led.set_low();
        info!("Finished playback of \"{}\"", current_track.name);

        // Pause 2 seconds between replays
        Timer::after(Duration::from_millis(2000)).await;
    }
}
