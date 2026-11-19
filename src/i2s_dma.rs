//! Hardware-timed I2S output to the MAX98357A amplifier, on the same pins as the
//! bit-banged driver in [`crate::max98357a`] (no rewiring needed):
//!
//! - BCLK -> PA3: TIM2 channel 4 PWM, 80 MHz / 78 = 1.0256 MHz, 50 % duty.
//! - LRC  -> PA4 and DIN -> PA10: GPIO outputs written by DMA. Every TIM2 update event
//!   (the BCLK falling edge) makes DMA1 channel 2 copy the next word of a circular buffer
//!   into GPIOA->BSRR, which sets LRC and DIN for that bit.
//!
//! The CPU only has to keep the circular buffer filled ahead of the DMA, so the I2S clocks
//! are perfectly regular whatever else the program does (blocking SPI, logging, interrupts).
//! The bit-banged driver cannot guarantee that: any CPU work between or inside frames shifts
//! the BCLK edges, and the MAX98357A then mutes. Use this driver whenever audio has to play
//! while the CPU does other work, such as talking to the NRF24 radio.
//!
//! Frame format matches the bit-banged driver: Philips I2S, 64 BCLK per frame, 16-bit
//! samples in 32-bit slots, the same sample in both slots, 16.026 kHz frame rate.
//! Requires the 80 MHz clock from [`crate::max98357a::config_80mhz`].

use core::ptr::addr_of_mut;
use core::sync::atomic::{AtomicBool, Ordering};

use embassy_stm32::dma::{Priority, Transfer, TransferOptions};
use embassy_stm32::gpio::{Level, Output, OutputType, Speed};
use embassy_stm32::peripherals::{DMA1_CH2, PA10, PA3, PA4, TIM2};
use embassy_stm32::time::Hertz;
use embassy_stm32::timer::low_level::{CountingMode, OutputCompareMode, OutputPolarity, Timer};
use embassy_stm32::timer::simple_pwm::{Ch4, PwmPin};
use embassy_stm32::timer::{Channel, UpDma};

/// GPIOA BSRR (Bit Set / Reset Register)
const GPIOA_BSRR: *mut u32 = 0x4800_0018 as *mut u32;
const LRC_HIGH: u32 = 1 << 4; // PA4
const LRC_LOW: u32 = 1 << (4 + 16);
const DIN_HIGH: u32 = 1 << 10; // PA10
const DIN_LOW: u32 = 1 << (10 + 16);

/// Timer clock (= SYSCLK with the APB1 prescaler at 1).
const TIMER_HZ: u32 = 80_000_000;
/// Timer ticks per BCLK period (same bit rate as the bit-banged driver).
const BIT_TICKS: u32 = 78;
/// BCLK periods (DMA words) per I2S frame.
const FRAME_WORDS: usize = 64;
/// Frames in the circular DMA buffer: 64 frames = 4 ms of audio, 16 KB of RAM.
/// [`I2sDma::fill`] must be called more often than this or old audio is replayed.
pub const RING_FRAMES: usize = 64;
const RING_WORDS: usize = RING_FRAMES * FRAME_WORDS;

/// Output frame (sample) rate in Hz: 80 MHz / (78 * 64) = 16,025.6 Hz.
pub const SAMPLE_RATE: u32 = 16_026;

/// The circular buffer read by the DMA. Only ever accessed through raw pointers.
static mut RING: [u32; RING_WORDS] = [0; RING_WORDS];
static TAKEN: AtomicBool = AtomicBool::new(false);

/// BSRR word for bit `bit` (0..63) of a frame carrying a zero data bit.
const fn silence_word(bit: usize) -> u32 {
    (if bit >= 32 { LRC_HIGH } else { LRC_LOW }) | DIN_LOW
}

/// Hardware-timed I2S output stream (TIM2 + DMA1 channel 2). Only one can exist.
pub struct I2sDma<'d> {
    _bclk: PwmPin<'d, TIM2, Ch4>,
    _lrc: Output<'d>,
    _din: Output<'d>,
    _timer: Timer<'d, TIM2>,
    dma: Transfer<'d>,
    ring: *mut u32,
    /// Next frame slot of the ring to fill
    write_frame: usize,
}

impl<'d> I2sDma<'d> {
    /// Set up the pins, timer and DMA, and start streaming silence.
    pub fn new(tim: TIM2, bclk: PA3, lrc: PA4, din: PA10, dma: DMA1_CH2) -> Self {
        assert!(!TAKEN.swap(true, Ordering::AcqRel), "I2sDma can only be created once");

        // Fill the whole ring with silent frames. Only the data words (bits 1..16 and
        // 33..48) are rewritten later; LRC and the zero bits stay as set here.
        let ring = addr_of_mut!(RING) as *mut u32;
        for i in 0..RING_WORDS {
            unsafe { ring.add(i).write_volatile(silence_word(i % FRAME_WORDS)) };
        }

        let lrc = Output::new(lrc, Level::Low, Speed::VeryHigh);
        let din = Output::new(din, Level::Low, Speed::VeryHigh);
        let bclk = PwmPin::new_ch4(bclk, OutputType::PushPull);

        // TIM2 counts 0..77 at 80 MHz. PWM mode 1 with active-low polarity keeps BCLK low
        // for counts 0..38 and high for 39..77, so the update event (count wraps to 0) is
        // the falling edge: exactly when I2S changes LRC and DIN.
        let timer = Timer::new(tim);
        timer.stop();
        timer.set_counting_mode(CountingMode::EdgeAlignedUp);
        timer.set_frequency(Hertz(TIMER_HZ / BIT_TICKS));
        timer.set_output_compare_mode(Channel::Ch4, OutputCompareMode::PwmMode1);
        timer.set_output_polarity(Channel::Ch4, OutputPolarity::ActiveLow);
        timer.set_compare_value(Channel::Ch4, BIT_TICKS / 2);
        timer.enable_channel(Channel::Ch4, true);

        // Circular DMA: ring -> GPIOA BSRR, one word per TIM2 update. No interrupts needed.
        let request = UpDma::<TIM2>::request(&dma);
        let mut options = TransferOptions::default();
        options.priority = Priority::VeryHigh;
        options.circular = true;
        options.half_transfer_ir = false;
        options.complete_transfer_ir = false;
        let dma = unsafe {
            Transfer::new_write_raw(
                dma,
                request,
                core::ptr::slice_from_raw_parts(ring as *const u32, RING_WORDS),
                GPIOA_BSRR,
                options,
            )
        };

        timer.enable_update_dma(true);
        timer.start();

        Self {
            _bclk: bclk,
            _lrc: lrc,
            _din: din,
            _timer: timer,
            dma,
            ring,
            write_frame: 1,
        }
    }

    /// Ring frame the DMA is currently sending (must not be overwritten).
    fn read_frame(&self) -> usize {
        let remaining = self.dma.get_remaining_transfers() as usize;
        ((RING_WORDS - remaining) % RING_WORDS) / FRAME_WORDS
    }

    /// Frames written but not yet sent, i.e. how much audio is queued (0..RING_FRAMES-1).
    /// A value that keeps dropping to 0 means `fill` is not called often enough.
    pub fn queued_frames(&self) -> usize {
        (self.write_frame + RING_FRAMES - self.read_frame() - 1) % RING_FRAMES
    }

    /// Write one frame slot with `sample` in both channels.
    fn write_frame_slot(&mut self, frame: usize, sample: i16) {
        let s = sample as u16;
        let base = unsafe { self.ring.add(frame * FRAME_WORDS) };
        let mut b = 0;
        while b < 16 {
            let din = if (s >> (15 - b)) & 1 != 0 { DIN_HIGH } else { DIN_LOW };
            unsafe {
                // Left slot: bits 1..16 (bit 0 is the I2S delay bit)
                base.add(1 + b).write_volatile(LRC_LOW | din);
                // Right slot: bits 33..48
                base.add(33 + b).write_volatile(LRC_HIGH | din);
            }
            b += 1;
        }
    }

    /// Fill every free frame of the ring, taking one sample per frame from `next_sample`
    /// (called 16,026 times per second in total). Returns the number of frames written.
    pub fn fill(&mut self, mut next_sample: impl FnMut() -> i16) -> usize {
        let mut written = 0;
        while self.write_frame != self.read_frame() {
            let sample = next_sample();
            self.write_frame_slot(self.write_frame, sample);
            self.write_frame = (self.write_frame + 1) % RING_FRAMES;
            written += 1;
        }
        written
    }
}
