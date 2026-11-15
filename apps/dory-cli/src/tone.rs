//! A sine tone this process plays on the default output, so a capture can prove it leaves out
//! Dory's own playback.

use std::cell::Cell;
use std::ptr::NonNull;

use block2::RcBlock;
use objc2_core_audio::{
    AudioDeviceCreateIOProcIDWithBlock, AudioDeviceDestroyIOProcID, AudioDeviceIOProcID,
    AudioDeviceStart, AudioDeviceStop, AudioObjectGetPropertyData, AudioObjectID,
    AudioObjectPropertyAddress, kAudioDevicePropertyNominalSampleRate,
    kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject,
};
use objc2_core_audio_types::{AudioBuffer, AudioBufferList, AudioTimeStamp};

const AMPLITUDE: f32 = 0.2;

pub struct Tone {
    device: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    _block: RcBlock<IoBlockFn>,
}

type IoBlockFn = dyn Fn(
    NonNull<AudioTimeStamp>,
    NonNull<AudioBufferList>,
    NonNull<AudioTimeStamp>,
    NonNull<AudioBufferList>,
    NonNull<AudioTimeStamp>,
);

impl Tone {
    pub fn play(hz: f64) -> Result<Self, String> {
        let device: AudioObjectID = get(
            kAudioObjectSystemObject as AudioObjectID,
            kAudioHardwarePropertyDefaultOutputDevice,
        )?;
        let rate: f64 = get(device, kAudioDevicePropertyNominalSampleRate)?;
        let step = std::f64::consts::TAU * hz / rate;
        let phase = Cell::new(0.0f64);
        let block: RcBlock<IoBlockFn> = RcBlock::new(
            move |_: NonNull<AudioTimeStamp>,
                  _: NonNull<AudioBufferList>,
                  _: NonNull<AudioTimeStamp>,
                  output: NonNull<AudioBufferList>,
                  _: NonNull<AudioTimeStamp>| {
                // SAFETY: the HAL passes this cycle's output list, with `mNumberBuffers` trailing
                // buffers of interleaved f32 that we may overwrite.
                unsafe {
                    let list = output.as_ptr();
                    let count = (*list).mNumberBuffers as usize;
                    let first = std::ptr::addr_of_mut!((*list).mBuffers).cast::<AudioBuffer>();
                    let mut end_phase = phase.get();
                    for b in 0..count {
                        let buffer = first.add(b).read();
                        let channels = buffer.mNumberChannels.max(1) as usize;
                        if buffer.mData.is_null() {
                            continue;
                        }
                        let samples = std::slice::from_raw_parts_mut(
                            buffer.mData.cast::<f32>(),
                            buffer.mDataByteSize as usize / 4,
                        );
                        let mut p = phase.get();
                        for frame in samples.chunks_exact_mut(channels) {
                            frame.fill(AMPLITUDE * p.sin() as f32);
                            p += step;
                        }
                        end_phase = p;
                    }
                    phase.set(end_phase % std::f64::consts::TAU);
                }
            },
        );
        let mut proc_id: AudioDeviceIOProcID = None;
        // SAFETY: valid out pointer and device; `Tone` keeps the block alive until after destroy.
        let status = unsafe {
            AudioDeviceCreateIOProcIDWithBlock(
                NonNull::from(&mut proc_id),
                device,
                None,
                RcBlock::as_ptr(&block),
            )
        };
        check(status, "AudioDeviceCreateIOProcIDWithBlock")?;
        let tone = Self {
            device,
            proc_id,
            _block: block,
        };
        // SAFETY: the IOProc was just created on this device.
        check(
            unsafe { AudioDeviceStart(device, tone.proc_id) },
            "AudioDeviceStart",
        )?;
        Ok(tone)
    }
}

impl Drop for Tone {
    fn drop(&mut self) {
        // SAFETY: created on this device in `play`, stopped and destroyed once.
        unsafe {
            AudioDeviceStop(self.device, self.proc_id);
            AudioDeviceDestroyIOProcID(self.device, self.proc_id);
        }
    }
}

fn check(status: i32, what: &str) -> Result<(), String> {
    if status == 0 {
        Ok(())
    } else {
        Err(format!("{what} failed: {status}"))
    }
}

fn get<T: Copy + Default>(object: AudioObjectID, selector: u32) -> Result<T, String> {
    let addr = AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut value = T::default();
    let mut size = size_of::<T>() as u32;
    // SAFETY: `value` is a writable `T` of `size` bytes, the plain C type of `selector`.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast(),
        )
    };
    check(status, "AudioObjectGetPropertyData")?;
    Ok(value)
}
