#![no_std]
#![no_main]

use defmt::info;
use embassy_executor::Spawner;
use embassy_stm32::gpio::{Level, Output, OutputType, Speed};
use embassy_stm32::rcc::*;
use embassy_stm32::time::Hertz;
use embassy_stm32::timer::low_level::CountingMode;
use embassy_stm32::timer::simple_pwm::{PwmPin, SimplePwm};
use embassy_time::{Duration, Timer};
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

// 80 MHz system clock
const SYSCLK_HZ: u32 = 80_000_000;

// Carrier PWM frequency: 100 kHz is ultrasonic (completely inaudible to eliminate buzzing)
const PWM_CARRIER_HZ: u32 = 100_000;

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // Configure system clock to 80 MHz via PLL from 4 MHz MSI
    let mut config = embassy_stm32::Config::default();
    config.rcc.pll = Some(Pll {
        source: PllSource::MSI,
        prediv: PllPreDiv::DIV1,
        mul: PllMul::MUL40, // 4 MHz * 40 = 160 MHz VCO
        divp: None,
        divq: None,
        divr: Some(PllRDiv::DIV2), // 160 MHz / 2 = 80 MHz SYSCLK
    });
    config.rcc.sys = Sysclk::PLL1_R;
    config.rcc.ahb_pre = AHBPrescaler::DIV1;
    config.rcc.apb1_pre = APBPrescaler::DIV1;
    config.rcc.apb2_pre = APBPrescaler::DIV1;

    let p = embassy_stm32::init(config);

    // Onboard green LED LD3 (PB3) as playback indicator
    let mut led = Output::new(p.PB3, Level::Low, Speed::Low);

    Timer::after(Duration::from_millis(500)).await;
    info!("===============================================");
    info!("=== Grove Speaker v1.1 Audio Player (80MHz) ===");
    info!("===============================================");
    info!("Output pin: PA8 (TIM1_CH1, Nucleo D9, PWM audio)");
    info!("Carrier frequency: {} Hz (ultrasonic)", PWM_CARRIER_HZ);

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
    info!("PWM max duty value: {}", max_duty);

    // Calculate CPU cycles per audio sample (cortex_m::asm::delay takes 3 cycles per unit)
    // For 8000 Hz sample rate at 80 MHz: 80,000,000 / 8,000 = 10,000 cycles (~3320 delay units after loop overhead)
    let delay_units_8k = ((SYSCLK_HZ / 8000) - 40) / 3;

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

        // Sample-accurate playback loop with zero jitter
        for &sample in current_track.data.iter() {
            // Scale 8-bit unsigned PCM sample (0..255) to PWM duty cycle
            let duty = ((sample as u32) * max_duty) / 255;
            pwm_ch1.set_duty_cycle(duty as u16);

            // Precise delay for exact 8,000 Hz sample timing
            cortex_m::asm::delay(delay_units_8k);
        }

        // Silence speaker after playback
        pwm_ch1.set_duty_cycle(0);
        led.set_low();
        info!("Finished playback of \"{}\"", current_track.name);

        // Pause before repeating or advancing to next track
        Timer::after(Duration::from_millis(1500)).await;
    }
}
