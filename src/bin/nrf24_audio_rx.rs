#![no_std]
#![no_main]

//! NRF24L01 audio receiver test: listens for audio packets and plays them on the speaker.
//!
//! Packet format (matches the `audio/*.raw` library): every 32-byte payload carries
//! 32 samples of 8-bit unsigned PCM (silence = 128) at 8,000 Hz, i.e. 4 ms of audio.
//! Any transmitter that sends such packets on channel 76 / address E7:E7:E7:E7:E7 works
//! (for example `nrf24_audio_file_tx` from the transmitter project).
//!
//! Radio (SPI1): SCK = PA5, MISO = PA6, MOSI = PA7, CSN = PA11, CE = PB0.
//! Speaker (I2S): BCLK = PA3, LRC = PA4, DIN = PA10 (same wiring as the other audio programs).
//!
//! The speaker is driven by the hardware-timed I2S driver ([`i2s_dma`]): TIM2 generates BCLK
//! and DMA writes LRC/DIN, so the I2S clocks stay perfectly regular while the CPU reads the
//! radio with ordinary blocking SPI calls. (With the bit-banged driver every radio access
//! disturbed the BCLK timing and the MAX98357A muted.)
//!
//! A jitter buffer absorbs packet timing variation: playback starts once `PREBUFFER`
//! samples have arrived and re-buffers after an underrun. Statistics are logged once per
//! second.
//!
//! [`i2s_dma`]: nucleo_l432kc_app::i2s_dma

use defmt::{error, info};
use embassy_executor::Spawner;
use embassy_stm32::gpio::{Level, Output, Speed};
use embassy_stm32::spi::{Config, Spi};
use embassy_stm32::time::Hertz;
use nucleo_l432kc_app::i2s_dma::{I2sDma, RING_FRAMES, SAMPLE_RATE};
use nucleo_l432kc_app::max98357a::config_80mhz;
use nucleo_l432kc_app::nrf24::{Nrf24, PAYLOAD_SIZE};
use {defmt_rtt as _, panic_probe as _};

/// Jitter buffer capacity in samples (must be a power of two): 2048 = 256 ms.
const BUF_SIZE: usize = 2048;
const MASK: usize = BUF_SIZE - 1;
/// Samples to collect before playback (re)starts: 512 = 64 ms.
const PREBUFFER: usize = 512;
/// Playback volume shift (0 = unchanged, 1 = 2x, ...). Samples are clipped.
const VOLUME_SHIFT: u32 = 0;
/// Poll the radio once every this many output frames: 16 = 1 ms (packets arrive every 4 ms).
const POLL_FRAMES: u32 = 16;
/// Length of the start-up beep in 8 kHz samples: 1,200 = 150 ms.
const BEEP_SAMPLES: u32 = 1_200;

/// Diagnostic mode (leave at 0 for normal operation):
/// 0 = play the received audio.
/// 1 = play a continuous 1 kHz tone and never touch the radio after start-up.
/// 2 = play a continuous 1 kHz tone while the radio is polled and drained as normal.
/// If 1 is audible but 2 is not, radio/SPI activity disturbs the amplifier electrically.
const DIAG: u8 = 0;

/// One period of a 1 kHz tone at 8 kHz.
const TONE_8K: [i16; 8] = [0, 21213, 30000, 21213, 0, -21213, -30000, -21213];

/// Jitter buffer and the 8 kHz -> 16 kHz sample feeder.
struct Player {
    data: [u8; BUF_SIZE],
    head: usize,
    count: usize,
    playing: bool,
    sample: i16,
    /// Every 8 kHz sample is sent in two consecutive 16 kHz frames
    second_frame: bool,
    tone_phase: usize,
    beep_left: u32,
    // statistics
    peak: u16,
    packets: u32,
    underruns: u32,
    overruns: u32,
}

impl Player {
    const fn new() -> Self {
        Self {
            data: [128; BUF_SIZE],
            head: 0,
            count: 0,
            playing: false,
            sample: 0,
            second_frame: true,
            tone_phase: 0,
            beep_left: BEEP_SAMPLES,
            peak: 0,
            packets: 0,
            underruns: 0,
            overruns: 0,
        }
    }

    /// Append a received packet; it is dropped (and counted) if the buffer is full.
    fn push(&mut self, packet: &[u8]) {
        self.packets += 1;
        if self.count + packet.len() > BUF_SIZE {
            self.overruns += 1;
            return;
        }
        for &s in packet {
            self.data[(self.head + self.count) & MASK] = s;
            self.count += 1;
        }
    }

    fn tone(&mut self) -> i16 {
        self.tone_phase = (self.tone_phase + 1) & 7;
        TONE_8K[self.tone_phase]
    }

    /// Sample for the next 16 kHz output frame.
    fn next_frame_sample(&mut self) -> i16 {
        self.second_frame = !self.second_frame;
        if self.second_frame {
            return self.sample;
        }

        self.sample = if self.beep_left > 0 {
            self.beep_left -= 1;
            self.tone()
        } else if DIAG != 0 {
            self.tone()
        } else {
            if !self.playing && self.count >= PREBUFFER {
                self.playing = true;
            } else if self.playing && self.count == 0 {
                self.playing = false;
                self.underruns += 1;
            }
            if self.playing {
                let raw = self.data[self.head];
                self.head = (self.head + 1) & MASK;
                self.count -= 1;
                let v = ((raw as i32) - 128) << (8 + VOLUME_SHIFT);
                v.clamp(-32768, 32767) as i16
            } else {
                0
            }
        };
        self.peak = self.peak.max(self.sample.unsigned_abs());
        self.sample
    }
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // 80 MHz SYSCLK: the I2S timer derives BCLK = 80 MHz / 78 from it
    let p = embassy_stm32::init(config_80mhz());

    // Onboard LED LD3 (PB3): lit while received audio is being played
    let mut led = Output::new(p.PB3, Level::Low, Speed::Low);
    let ce = Output::new(p.PB0, Level::Low, Speed::Low);
    let csn = Output::new(p.PA11, Level::High, Speed::High);

    info!("=======================================================");
    info!("=== NRF24L01 Audio Receiver -> MAX98357A Speaker    ===");
    info!("=======================================================");
    info!("Expecting 32-byte packets of 8-bit unsigned PCM @ 8 kHz on channel 76.");

    let mut spi_config = Config::default();
    spi_config.frequency = Hertz(4_000_000);
    // Slow edges: less noise coupled into the neighbouring I2S wires
    spi_config.rise_fall_speed = Speed::Low;
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

    // Hardware-timed I2S: from here on the clocks run on their own (TIM2 + DMA)
    let mut i2s = I2sDma::new(p.TIM2, p.PA3, p.PA4, p.PA10, p.DMA1_CH2);
    info!("I2S output running (TIM2 + DMA). DIAG = {}", DIAG);

    let mut player = Player::new();
    let mut payload = [0u8; PAYLOAD_SIZE];
    let mut frames_since_poll: u32 = 0;
    let mut frames_since_log: u32 = 0;
    let mut min_queued = RING_FRAMES;

    loop {
        // Refill the frames the DMA has sent since the last pass
        min_queued = min_queued.min(i2s.queued_frames());
        let written = i2s.fill(|| player.next_frame_sample()) as u32;
        frames_since_poll += written;
        frames_since_log += written;

        // Read every packet waiting in the radio FIFO (blocking SPI is fine now)
        if DIAG != 1 && frames_since_poll >= POLL_FRAMES {
            frames_since_poll = 0;
            while radio.read_packet(&mut payload) {
                if DIAG == 0 {
                    player.push(&payload);
                } else {
                    player.packets += 1;
                }
            }
        }

        if player.playing != led.is_set_high() {
            led.set_level(if player.playing { Level::High } else { Level::Low });
        }

        if frames_since_log >= SAMPLE_RATE {
            frames_since_log = 0;
            info!(
                "Packets: {}, buffered: {} samples, peak: {}, underruns: {}, overruns: {}, min I2S queue: {}/{} frames",
                player.packets,
                player.count,
                player.peak,
                player.underruns,
                player.overruns,
                min_queued,
                RING_FRAMES - 1
            );
            player.packets = 0;
            player.peak = 0;
            min_queued = RING_FRAMES;
        }
    }
}
