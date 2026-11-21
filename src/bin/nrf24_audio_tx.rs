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
//! The I2S clocks are bit-banged at 16 kHz and must run continuously. The mic is decimated
//! 2:1 (average of two frames) to 8 kHz and the radio is driven without waiting: one packet
//! is started every 4 ms and its ACK result is collected before the next one. Starting a
//! packet costs ~150 us during which no I2S frames are generated, so fewer than 32 samples
//! are taken from the mic per packet (measured each packet from the DWT cycle counter) and
//! the remainder is filled by repeating the last sample. This keeps the average rate at
//! 8 kHz so the receiver's jitter buffer neither fills nor drains.

use core::cell::Cell;
use cortex_m::peripheral::DWT;
use defmt::{error, info};
use embassy_executor::Spawner;
use embassy_stm32::gpio::{Level, Output, Speed};
use embassy_stm32::spi::{Config, Spi};
use embassy_stm32::time::Hertz;
use nucleo_l432kc_app::max98357a::{config_80mhz, Max98357a, SYSCLK_HZ};
use nucleo_l432kc_app::nrf24::{Nrf24, PAYLOAD_SIZE};
use {defmt_rtt as _, panic_probe as _};

/// Digital gain applied to the microphone (raw INMP441 speech is very quiet).
const GAIN: i32 = 64;
/// Target packet period in CPU cycles (4 ms).
const PERIOD_CYCLES: u32 = SYSCLK_HZ / 250;
/// Cycles of one 8 kHz sample (2 I2S frames of 4,992 cycles).
const SAMPLE_CYCLES: u32 = 2 * 4_992;

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_stm32::init(config_80mhz());

    let mut led = Output::new(p.PB3, Level::Low, Speed::Low);
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
            embassy_time::Timer::after_millis(100).await;
        }
    }
    radio.config_tx().await;
    info!("Radio configured as transmitter on channel 76.");

    // Clock-only use of the amplifier pins: PA10 (DIN) just outputs silence
    let mut mic = Max98357a::new_with_mic(p.PA3, p.PA4, p.PA10, p.PA1);

    // Keep clocks running so the INMP441 finishes its startup filter, and detect whether
    // L/R is tied to GND (left) or VDD (right).
    info!("Warming up microphone (350 ms)...");
    let mut left_warmup = 0u32;
    let mut right_warmup = 0u32;
    for _ in 0..(16_000 * 350) / 1000 {
        let (l, r) = mic.exchange_frame(0, 0);
        if l != 0 {
            left_warmup += 1;
        }
        if r != 0 {
            right_warmup += 1;
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
    let peak = Cell::new(0i32);

    let mut payload = [128u8; PAYLOAD_SIZE];
    let mut sent_ok: u32 = 0;
    let mut no_ack: u32 = 0;
    let mut in_flight = false;
    let mut packets: u32 = 0;
    let mut real_samples_total: u32 = 0;
    // Cost of one radio exchange in cycles; measured, starts with a conservative guess
    let mut send_cost: u32 = 16_000;

    loop {
        let packet_start = DWT::cycle_count();

        // 1. Collect mic samples (2 I2S frames each) until the time budget of this packet
        //    is used up (period minus the measured radio cost), at most 32 samples.
        let mut n = 0usize;
        while n < PAYLOAD_SIZE
            && (DWT::cycle_count().wrapping_sub(packet_start))
                .saturating_add(SAMPLE_CYCLES + send_cost)
                <= PERIOD_CYCLES
        {
            let mut acc = 0i32;
            for _ in 0..2 {
                let (l, r) = mic.exchange_frame(0, 0);
                let x = (if use_right { r } else { l }) as i32;
                let y = x - prev_x + ((prev_y * 253) >> 8);
                prev_x = x;
                prev_y = y;
                let out = (y * GAIN).clamp(-32767, 32767);
                acc += out;
                if out.abs() > peak.get() {
                    peak.set(out.abs());
                }
            }
            // Average of two frames = simple anti-alias filter; 16-bit -> unsigned 8-bit
            payload[n] = (((acc >> 1) >> 8) + 128).clamp(0, 255) as u8;
            n += 1;
        }
        real_samples_total += n as u32;
        // Pad the remaining samples by repeating the last one (or silence)
        let pad = if n > 0 { payload[n - 1] } else { 128 };
        for s in payload[n..].iter_mut() {
            *s = pad;
        }

        // 2. Collect the previous packet's result, then start the next one (non-blocking)
        let t0 = DWT::cycle_count();
        if in_flight {
            match radio.finish_send() {
                Some(true) => sent_ok += 1,
                _ => no_ack += 1,
            }
        }
        radio.start_send(&payload);
        send_cost = DWT::cycle_count().wrapping_sub(t0);
        in_flight = true;
        packets += 1;

        // 3. Statistics once per second (250 packets)
        if packets >= 250 {
            led.set_level(if sent_ok > no_ack { Level::High } else { Level::Low });
            info!(
                "Packets: {} acked, {} lost/no-ack | peak {} / 32767 | avg {} real samples/packet | radio {} cycles | late bits {}",
                sent_ok,
                no_ack,
                peak.get(),
                real_samples_total / packets,
                send_cost,
                mic.late_bits()
            );
            packets = 0;
            sent_ok = 0;
            no_ack = 0;
            real_samples_total = 0;
            peak.set(0);
        }
    }
}
