//! Hardware-timed I2S master + INMP441 microphone capture on the shared BCLK/WS pins.
//!
//! - BCLK -> PA3: TIM2 channel 4 PWM, 80 MHz / 78 = 1.0256 MHz, 50 % duty.
//! - LRC  -> PA4 and DIN -> PA10: GPIO outputs written by DMA1 channel 2 from a one-frame
//!   circular buffer on every TIM2 update event (BCLK falling edge). DIN stays silent; the
//!   MAX98357A on the same clock lines simply plays nothing.
//! - SD   <- PA1: the whole GPIOA input register is copied by DMA1 channel 1 into a circular
//!   buffer on every TIM2 channel 3 compare event (in the middle of the BCLK high phase).
//!
//! Because both directions are done by DMA the clocks never glitch, whatever the CPU does
//! (blocking SPI, logging, ...). The CPU just decodes finished frames from the capture ring
//! with [`I2sMicDma::poll_frame`]; it must do so at least once per `CAP_FRAMES` frames (8 ms).
//!
//! Frame format: Philips I2S, 64 BCLK per frame, 16 audio bits used per 32-bit slot,
//! 16.026 kHz frame rate. Requires the 80 MHz clock from [`crate::max98357a::config_80mhz`].

use core::ptr::addr_of_mut;
use core::sync::atomic::{AtomicBool, Ordering};

use embassy_stm32::dma::{Priority, Transfer, TransferOptions};
use embassy_stm32::gpio::{Input, Level, Output, OutputType, Pull, Speed};
use embassy_stm32::peripherals::{DMA1_CH1, DMA1_CH2, PA1, PA10, PA3, PA4, TIM2};
use embassy_stm32::time::Hertz;
use embassy_stm32::timer::low_level::{CountingMode, OutputCompareMode, OutputPolarity, Timer};
use embassy_stm32::timer::simple_pwm::{Ch4, PwmPin};
use embassy_stm32::timer::{Ch3Dma, Channel, UpDma};

const GPIOA_BSRR: *mut u32 = 0x4800_0018 as *mut u32;
const GPIOA_IDR: *mut u16 = 0x4800_0010 as *mut u16;
const LRC_HIGH: u32 = 1 << 4;
const LRC_LOW: u32 = 1 << (4 + 16);
const DIN_LOW: u32 = 1 << (10 + 16);

const TIMER_HZ: u32 = 80_000_000;
const BIT_TICKS: u32 = 78;
/// Timer count at which SD is sampled (BCLK is high for counts 39..77).
const SAMPLE_TICK: u32 = 60;
const FRAME_WORDS: usize = 64;
/// Capture ring size in frames (128 frames = 8 ms, 16 KB).
pub const CAP_FRAMES: usize = 128;
const CAP_WORDS: usize = CAP_FRAMES * FRAME_WORDS;

/// Output frame (sample) rate in Hz.
pub const SAMPLE_RATE: u32 = 16_026;

static mut OUT_RING: [u32; FRAME_WORDS] = [0; FRAME_WORDS];
static mut CAP_RING: [u16; CAP_WORDS] = [0; CAP_WORDS];
static TAKEN: AtomicBool = AtomicBool::new(false);

pub struct I2sMicDma<'d> {
    _bclk: PwmPin<'d, TIM2, Ch4>,
    _lrc: Output<'d>,
    _din: Output<'d>,
    _sd: Input<'d>,
    _timer: Timer<'d, TIM2>,
    _out_dma: Transfer<'d>,
    cap_dma: Transfer<'d>,
    cap: *const u16,
    /// Word index in the capture ring where the next unread frame starts
    next_word: usize,
    overruns: u32,
}

impl<'d> I2sMicDma<'d> {
    /// Start the clocks and the capture. Call [`I2sMicDma::sync`] after ~20 ms before reading.
    pub fn new(
        tim: TIM2,
        bclk: PA3,
        lrc: PA4,
        din: PA10,
        sd: PA1,
        out_dma: DMA1_CH2,
        cap_dma: DMA1_CH1,
    ) -> Self {
        assert!(!TAKEN.swap(true, Ordering::AcqRel), "I2sMicDma can only be created once");

        let out_ring = addr_of_mut!(OUT_RING) as *mut u32;
        for i in 0..FRAME_WORDS {
            let ws = if i >= 32 { LRC_HIGH } else { LRC_LOW };
            unsafe { out_ring.add(i).write_volatile(ws | DIN_LOW) };
        }
        let cap = addr_of_mut!(CAP_RING) as *mut u16;

        let lrc = Output::new(lrc, Level::Low, Speed::VeryHigh);
        let din = Output::new(din, Level::Low, Speed::VeryHigh);
        let sd = Input::new(sd, Pull::None);
        let bclk = PwmPin::new_ch4(bclk, OutputType::PushPull);

        let timer = Timer::new(tim);
        timer.stop();
        timer.set_counting_mode(CountingMode::EdgeAlignedUp);
        timer.set_frequency(Hertz(TIMER_HZ / BIT_TICKS));
        timer.set_output_compare_mode(Channel::Ch4, OutputCompareMode::PwmMode1);
        timer.set_output_polarity(Channel::Ch4, OutputPolarity::ActiveLow);
        timer.set_compare_value(Channel::Ch4, BIT_TICKS / 2);
        timer.enable_channel(Channel::Ch4, true);
        // Channel 3 is only used as a DMA trigger (no pin)
        timer.set_compare_value(Channel::Ch3, SAMPLE_TICK);

        let mut options = TransferOptions::default();
        options.priority = Priority::VeryHigh;
        options.circular = true;
        options.half_transfer_ir = false;
        options.complete_transfer_ir = false;

        let out_req = UpDma::<TIM2>::request(&out_dma);
        let out_dma = unsafe {
            Transfer::new_write_raw(
                out_dma,
                out_req,
                core::ptr::slice_from_raw_parts(out_ring as *const u32, FRAME_WORDS),
                GPIOA_BSRR,
                options,
            )
        };

        let cap_req = Ch3Dma::<TIM2>::request(&cap_dma);
        let cap_dma = unsafe {
            Transfer::new_read_raw(
                cap_dma,
                cap_req,
                GPIOA_IDR,
                core::ptr::slice_from_raw_parts_mut(cap, CAP_WORDS),
                options,
            )
        };

        timer.enable_update_dma(true);
        timer.set_cc_dma_enable_state(Channel::Ch3, true);
        timer.start();

        Self {
            _bclk: bclk,
            _lrc: lrc,
            _din: din,
            _sd: sd,
            _timer: timer,
            _out_dma: out_dma,
            cap_dma,
            cap,
            next_word: 0,
            overruns: 0,
        }
    }

    /// Word the capture DMA will write next.
    fn write_pos(&self) -> usize {
        (CAP_WORDS - self.cap_dma.get_remaining_transfers() as usize) % CAP_WORDS
    }

    #[inline(always)]
    fn word(&self, i: usize) -> u16 {
        unsafe { self.cap.add(i % CAP_WORDS).read_volatile() }
    }

    /// Find the frame boundary in the capture ring from the WS line (PA4, falling edge =
    /// first bit of a frame). Needs the capture to have run for at least one full ring
    /// (8 ms) first. Returns false if no WS edge was found (clocks not running).
    pub fn sync(&mut self) -> bool {
        for i in 0..CAP_WORDS {
            let prev = self.word(i + CAP_WORDS - 1);
            let cur = self.word(i);
            if prev & (1 << 4) != 0 && cur & (1 << 4) == 0 {
                self.next_word = i;
                // Skip everything before the write position: start with the next frame
                let pos = self.write_pos();
                let ahead = (pos + CAP_WORDS - i) % CAP_WORDS;
                self.next_word = (i + (ahead / FRAME_WORDS) * FRAME_WORDS) % CAP_WORDS;
                return true;
            }
        }
        false
    }

    /// Number of times the CPU fell more than one ring behind (frames were skipped).
    pub fn overruns(&self) -> u32 {
        self.overruns
    }

    /// Returns the next finished frame as `(left, right)` 16-bit samples, or `None` if none
    /// is ready yet.
    pub fn poll_frame(&mut self) -> Option<(i16, i16)> {
        let pos = self.write_pos();
        let mut avail = (pos + CAP_WORDS - self.next_word) % CAP_WORDS;
        if avail < FRAME_WORDS {
            return None;
        }
        // Too far behind: the DMA may have overwritten the oldest frames. Skip ahead.
        if avail > CAP_WORDS - 2 * FRAME_WORDS {
            self.overruns += 1;
            let skip = (avail / FRAME_WORDS - 1) * FRAME_WORDS;
            self.next_word = (self.next_word + skip) % CAP_WORDS;
            avail -= skip;
            let _ = avail;
        }
        let base = self.next_word;
        let mut left = 0u16;
        let mut right = 0u16;
        for b in 0..16 {
            left = (left << 1) | ((self.word(base + 1 + b) >> 1) & 1);
            right = (right << 1) | ((self.word(base + 33 + b) >> 1) & 1);
        }
        self.next_word = (base + FRAME_WORDS) % CAP_WORDS;
        Some((left as i16, right as i16))
    }
}
