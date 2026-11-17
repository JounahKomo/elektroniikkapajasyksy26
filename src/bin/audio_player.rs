#![no_std]
#![no_main]

use defmt::info;
use embassy_executor::Spawner;
use embassy_stm32::gpio::{Level, Output, OutputType, Speed};
use embassy_stm32::rcc::MSIRange;
use embassy_stm32::time::Hertz;
use embassy_stm32::timer::low_level::CountingMode;
use embassy_stm32::timer::simple_pwm::{PwmPin, SimplePwm};
use embassy_time::{Duration, Instant, Timer};
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

pub const ALL_TRACKS: &[Track] = &[TRACK_CHIME, TRACK_BELL, TRACK_ROGER_BEEP];

// =============================================================================
// PLAYBACK CONFIGURATION:
// Choose an audio file to play, or set PLAY_ALL_TRACKS = true to cycle through all!
// =============================================================================
const SELECTED_TRACK: Track = TRACK_CHIME;
const PLAY_ALL_TRACKS: bool = false;

// Carrier PWM frequency (64 kHz is well above human audible range ~20 kHz)
const PWM_CARRIER_HZ: u32 = 64_000;

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // Configure system clock to 48 MHz MSI for high-resolution PWM & audio timing
    let mut config = embassy_stm32::Config::default();
    config.rcc.msi = Some(MSIRange::RANGE48M);
    let p = embassy_stm32::init(config);

    // Onboard green LED LD3 (PB3) as playback indicator
    let mut led = Output::new(p.PB3, Level::Low, Speed::Low);

    Timer::after(Duration::from_millis(500)).await;
    info!("===============================================");
    info!("=== Grove Speaker v1.1 Audio Player         ===");
    info!("===============================================");
    info!("Output pin: PA8 (TIM1_CH1, Nucleo D9, PWM audio)");
    info!("Carrier frequency: {} Hz", PWM_CARRIER_HZ);

    // Configure PA8 as TIM1 CH1 PWM output
    let ch1 = PwmPin::new_ch1(p.PA8, OutputType::PushPull);
    let mut pwm = SimplePwm::new(
        p.TIM1,
        Some(ch1),
        None,
        None,
        None,
        Hertz(PWM_CARRIER_HZ),
        CountingMode::EdgeAlignedUp,
    );

    let mut pwm_ch1 = pwm.ch1();
    pwm_ch1.enable();
    pwm_ch1.set_duty_cycle(0);

    let max_duty = pwm_ch1.max_duty_cycle() as u32;
    info!("PWM max duty cycle value: {}", max_duty);

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

        // Sample interval in microseconds (e.g. 125 us for 8000 Hz)
        let sample_interval_us = 1_000_000u64 / current_track.sample_rate as u64;
        let start_time = Instant::now();

        for (i, &sample) in current_track.data.iter().enumerate() {
            // Target timestamp for this sample (avoids cumulative timing drift)
            let target = start_time + Duration::from_micros(i as u64 * sample_interval_us);

            // Scale 8-bit unsigned PCM sample (0..255) to PWM duty cycle
            let duty = ((sample as u32) * max_duty) / 255;
            pwm_ch1.set_duty_cycle(duty as u16);

            // Wait until the sample timestamp
            while Instant::now() < target {
                core::hint::spin_loop();
            }
        }

        // Silence speaker after playback
        pwm_ch1.set_duty_cycle(0);
        led.set_low();
        info!("Finished playback of \"{}\"", current_track.name);

        // Pause before repeating or advancing to next track
        Timer::after(Duration::from_millis(1500)).await;
    }
}
