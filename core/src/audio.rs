//! Concatenation and playback, replacing `sox` and `afplay`.

use std::fs::File;
use std::num::NonZero;
use std::path::Path;

use rodio::buffer::SamplesBuffer;
use rodio::{DeviceSinkBuilder, Player};

/// Decoded audio ready to play.
#[derive(Debug, Clone, PartialEq)]
pub struct Clip {
    pub channels: u16,
    pub sample_rate: u32,
    pub samples: Vec<f32>,
}

fn read_wav(path: &Path) -> Result<Clip, String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let spec = reader.spec();
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().filter_map(Result::ok).collect(),
        hound::SampleFormat::Int => {
            let scale = (1_i64 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().filter_map(Result::ok).map(|s| s as f32 / scale).collect()
        }
    };
    Ok(Clip { channels: spec.channels, sample_rate: spec.sample_rate, samples })
}

/// Joins wav files in order. The first file sets the format; a chunk with a
/// different one is skipped (reported in the error list) instead of being
/// played at the wrong speed.
pub fn concat_wavs(paths: &[impl AsRef<Path>]) -> (Option<Clip>, Vec<String>) {
    let mut clip: Option<Clip> = None;
    let mut errors = Vec::new();
    for path in paths {
        match read_wav(path.as_ref()) {
            Ok(next) => match clip.as_mut() {
                None => clip = Some(next),
                Some(c) if c.channels == next.channels && c.sample_rate == next.sample_rate => {
                    c.samples.extend(next.samples)
                }
                Some(_) => errors.push(format!("{}: format differs, skipped", path.as_ref().display())),
            },
            Err(e) => errors.push(e),
        }
    }
    (clip, errors)
}

/// Plays a clip on the default output device and blocks until it ends.
/// `lock` serializes playback across processes (dispatcher and `say`).
pub fn play(clip: Clip, lock: &Path) -> Result<(), String> {
    let lock_file = File::create(lock).map_err(|e| format!("playback lock: {e}"))?;
    lock_file.lock().map_err(|e| format!("playback lock: {e}"))?;

    let channels = NonZero::new(clip.channels).ok_or("clip has no channels")?;
    let rate = NonZero::new(clip.sample_rate).ok_or("clip has no sample rate")?;
    let mut sink = DeviceSinkBuilder::open_default_sink().map_err(|e| format!("audio output: {e}"))?;
    sink.log_on_drop(false);
    let player = Player::connect_new(sink.mixer());
    player.append(SamplesBuffer::new(channels, rate, clip.samples));
    player.sleep_until_end();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_wav(path: &Path, rate: u32, samples: &[i16]) {
        let spec = hound::WavSpec { channels: 1, sample_rate: rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for s in samples {
            writer.write_sample(*s).unwrap();
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn concatenates_in_order_and_skips_mismatched_chunks() {
        let dir = TempDir::new().unwrap();
        let (a, b, c) = (dir.path().join("a.wav"), dir.path().join("b.wav"), dir.path().join("c.wav"));
        write_wav(&a, 24000, &[0, 16384]);
        write_wav(&b, 22050, &[1]);
        write_wav(&c, 24000, &[-16384]);
        let (clip, errors) = concat_wavs(&[&a, &b, &c, &dir.path().join("missing.wav")]);
        let clip = clip.unwrap();
        assert_eq!(clip.sample_rate, 24000);
        assert_eq!(clip.samples, vec![0.0, 0.5, -0.5]);
        assert_eq!(errors.len(), 2);
    }
}
