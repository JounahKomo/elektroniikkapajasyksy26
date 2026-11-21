#![no_std]
#![no_main]

//! NRF24L01 audio transmitter test: records the INMP441 microphone and streams it over the
//! radio. Compatible with `nrf24_audio_rx`.
//!
//! Packet format (what the receiver expects): every 32-byte payload carries 32 samples of
//! 8-bit unsigned PCM (silence = 128) at 8,000 Hz, i.e. 4 ms of audio, on channel 76 /
//! address E7:E7:E7:E7:E7.
//!
//! Radio (SPI1): SCK = PA5, MISO = PA6, MOSI = PA7, CSN = PA11, CE = PB0.
//! Microphone (I2S): SCK = PA3, WS = PA4, SD = PA1 (L/R pin to GND or 3.3V, auto-detected).
//! Status LED: PB3 (LD3), lit while the radio is acknowledging packets.
//!
//! The microphone is captured by hardware: TIM2 generates BCLK/WS and DMA samples the mic
//! data line ([`i2s_mic_dma`]), so radio SPI traffic can never disturb the audio clocks
//! and no samples are lost. The 16 kHz mic stream is averaged 2:1 down to 8 kHz and every
//! 32 samples are sent as one packet (the non-blocking `start_send` / `finish_send`).
//!
//! [`i2s_mic_dma`]: nucleo_l432kc_app::i2s_mic_dma

use defmt::{error, info};
use embassy_executor::Spawner;
use embassy_stm32::gpio::{Input, Level, Output, Pull, Speed};
use embassy_stm32::spi::{Config, Spi};
use embassy_stm32::time::Hertz;
use embassy_time::Timer;
use nucleo_l432kc_app::i2s_mic_dma::I2sMicDma;
use nucleo_l432kc_app::max98357a::config_80mhz;
use nucleo_l432kc_app::nrf24::{Nrf24, PAYLOAD_SIZE};
use {defmt_rtt as _, panic_probe as _};

/// Digital gain applied to the microphone (raw INMP441 speech is very quiet).
const GAIN: i32 = 32;
/// Transmit only while the push-to-talk button (PB1 to GND, internal pull-up) is held.
/// An always-open microphone next to the receiver's speaker howls (acoustic feedback).
/// Set to false to stream continuously, e.g. when the boards are far apart.
const REQUIRE_PTT: bool = false;
/// Noise gate: the microphone is muted until the signal envelope exceeds `GATE_OPEN`, and
/// muted again when it falls below `GATE_CLOSE` (units: 16-bit sample after gain, full
/// scale 32767). Speech after 32x gain is in the thousands; room noise and faint speaker
/// sound picked up from the receiver are well below. Raise both if feedback still builds,
/// lower them if quiet speech gets cut off. Set `GATE_OPEN` to 0 to disable the gate.
const GATE_OPEN: i32 = 1_500;
const GATE_CLOSE: i32 = 700;
/// PTT state must be stable for this many 4 ms packets (debounce).
const PTT_DEBOUNCE: u8 = 5;

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_stm32::init(config_80mhz());

    let mut led = Output::new(p.PB3, Level::Low, Speed::Low);
    let ptt = Input::new(p.PB1, Pull::Up);
    let ce = Output::new(p.PB0, Level::Low, Speed::Low);
    let csn = Output::new(p.PA11, Level::High, Speed::High);

    info!("=======================================================");
    info!("=== INMP441 Microphone -> NRF24L01 Audio Transmitter ===");
    info!("=======================================================");
    info!("Sending 32-byte packets of 8-bit unsigned PCM @ 8 kHz on channel 76.");

    let mut spi_config = Config::default();
    spi_config.frequency = Hertz(4_000_000);
    let spi = Spi::new_blocking(p.SPI1, p.PA5, p.PA7, p.PA6, spi_config);
    let mut radio = Nrf24::new(spi, ce, csn);

    if !radio.init().await {
        error!("Failed to initialize NRF24L01! Check wiring, 3.3V power, and GND.");
        loop {
            led.toggle();
            Timer::after_millis(100).await;
        }
    }
    radio.config_tx().await;
    info!("Radio configured as transmitter on channel 76.");

    // Hardware I2S clocks + microphone capture (TIM2 + DMA1 channels 1 and 2)
    let mut mic = I2sMicDma::new(p.TIM2, p.PA3, p.PA4, p.PA10, p.PA1, p.DMA1_CH2, p.DMA1_CH1);

    // Let the capture ring fill, then locate the frame boundary
    Timer::after_millis(20).await;
    if !mic.sync() {
        error!("No I2S word-select edge found in the capture buffer!");
    }

    // Warm up (INMP441 start-up filter takes ~350 ms) and detect L/R: GND = left, VDD = right
    info!("Warming up microphone (350 ms)...");
    let mut left_warmup = 0u32;
    let mut right_warmup = 0u32;
    let mut frames = 0u32;
    while frames < 16_000 * 350 / 1000 {
        match mic.poll_frame() {
            Some((l, r)) => {
                frames += 1;
                if l != 0 {
                    left_warmup += 1;
                }
                if r != 0 {
                    right_warmup += 1;
                }
            }
            None => Timer::after_micros(500).await,
        }
    }
    if left_warmup == 0 && right_warmup == 0 {
        error!("Microphone SD line (PA1) never left 0: no data from the INMP441!");
        error!("Check: SCK -> PA3, WS -> PA4, SD -> PA1, VDD = 3.3V, GND, and the L/R pin.");
    }
    let use_right = right_warmup > left_warmup;
    info!(
        "Microphone channel: {} (activity: Left={}, Right={})",
        if use_right { "Right" } else { "Left" },
        left_warmup,
        right_warmup
    );

    // DC-blocking high-pass filter state: y[n] = x[n] - x[n-1] + 253/256 * y[n-1]
    let mut prev_x = 0i32;
    let mut prev_y = 0i32;
    let mut peak = 0i32;
    // Noise gate state: envelope follower and gate gain (0..=256)
    let mut env = 0i32;
    let mut gate_open = GATE_OPEN == 0;
    let mut gate_gain = if GATE_OPEN == 0 { 256i32 } else { 0 };
    let mut open_samples: u32 = 0;

    let mut payload = [128u8; PAYLOAD_SIZE];
    let mut n = 0usize; // samples in the current packet
    let mut half = false; // first of the two frames of an 8 kHz sample already seen
    let mut acc = 0i32;
    let mut sent_ok: u32 = 0;
    let mut no_ack: u32 = 0;
    let mut in_flight = false;
    let mut talking = !REQUIRE_PTT;
    let mut ptt_count: u8 = 0;
    let mut packets: u32 = 0;

    loop {
        let Some((l, r)) = mic.poll_frame() else {
            // Nothing new: yield briefly (a frame takes 62 us)
            Timer::after_micros(100).await;
            continue;
        };

        let x = (if use_right { r } else { l }) as i32;
        let y = x - prev_x + ((prev_y * 253) >> 8);
        prev_x = x;
        prev_y = y;
        let out = (y * GAIN).clamp(-32767, 32767);
        peak = peak.max(out.abs());
        acc += out;

        if !half {
            half = true;
            continue;
        }
        half = false;
        // Average of two frames = simple anti-alias filter; 16-bit -> unsigned 8-bit
        let mut v = acc >> 1;
        acc = 0;
        if GATE_OPEN != 0 {
            let a = v.abs();
            // Instant attack, release of about 60 ms (1/512 per sample at 8 kHz)
            env = if a > env { a } else { env - (env >> 9) };
            if !gate_open && env > GATE_OPEN {
                gate_open = true;
            } else if gate_open && env < GATE_CLOSE {
                gate_open = false;
            }
            // Gain ramps: open in ~1 ms, close in ~30 ms, so there are no clicks
            gate_gain = if gate_open {
                (gate_gain + 32).min(256)
            } else {
                (gate_gain - 8).max(0)
            };
            v = (v * gate_gain) >> 8;
            if gate_gain > 0 {
                open_samples += 1;
            }
        }
        payload[n] = ((v >> 8) + 128).clamp(0, 255) as u8;
        n += 1;
        if n < PAYLOAD_SIZE {
            continue;
        }
        n = 0;

        // Push-to-talk with debounce, checked once per packet
        if REQUIRE_PTT {
            if ptt.is_low() != talking {
                ptt_count += 1;
                if ptt_count >= PTT_DEBOUNCE {
                    talking = !talking;
                    ptt_count = 0;
                    info!("PTT {}", if talking { "pressed: transmitting" } else { "released: idle" });
                }
            } else {
                ptt_count = 0;
            }
            if !talking {
                if in_flight {
                    let _ = radio.finish_send();
                    in_flight = false;
                }
                continue;
            }
        }

        // Packet complete: collect the previous result, then start the next send
        if in_flight {
            match radio.finish_send() {
                Some(true) => sent_ok += 1,
                _ => no_ack += 1,
            }
        }
        radio.start_send(&payload);
        in_flight = true;
        packets += 1;

        if packets >= 250 {
            led.set_level(if talking && sent_ok > no_ack { Level::High } else { Level::Low });
            info!(
                "Packets: {} acked, {} lost/no-ack | peak {} / 32767 | gate open {}% | capture overruns {}",
                sent_ok,
                no_ack,
                peak,
                open_samples / 80,
                mic.overruns()
            );
            packets = 0;
            sent_ok = 0;
            no_ack = 0;
            peak = 0;
            open_samples = 0;
        }
    }
}
