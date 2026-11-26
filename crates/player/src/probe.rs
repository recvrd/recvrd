//! Technical facts about one audio file.

use std::path::Path;

use symphonia::core::{
    formats::FormatOptions, io::MediaSourceStream, meta::MetadataOptions, probe::Hint,
};

/// Technical facts from one audio file header.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioFacts {
    pub container: Option<String>,
    pub codec: Option<String>,
    pub bitrate_kbps: Option<u32>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    pub bits_per_sample: Option<u32>,
    pub duration_secs: Option<f64>,
}

/// Read the container, codec and stream parameters of one file.
///
/// The probe reads headers only. It does not decode the audio.
pub fn probe_audio(path: &Path) -> Result<AudioFacts, String> {
    let size = std::fs::metadata(path)
        .map_err(|error| format!("read {}: {error}", path.display()))?
        .len();
    let file =
        std::fs::File::open(path).map_err(|error| format!("open {}: {error}", path.display()))?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|value| value.to_str()) {
        hint.with_extension(extension);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|error| format!("probe {}: {error}", path.display()))?;
    let track = probed
        .format
        .default_track()
        .ok_or_else(|| format!("no audio track in {}", path.display()))?;
    let params = &track.codec_params;
    let sample_rate = params.sample_rate;
    let duration_secs = match (params.n_frames, sample_rate) {
        (Some(frames), Some(rate)) if rate > 0 => Some(frames as f64 / rate as f64),
        _ => None,
    };
    let bitrate_kbps = duration_secs
        .filter(|duration| *duration > 0.0)
        .map(|duration| (size as f64 * 8.0 / duration / 1000.0).round() as u32);
    let codec = symphonia::default::get_codecs()
        .get_codec(params.codec)
        .map(|codec| codec.short_name.to_string())
        .filter(|name| !name.is_empty());
    Ok(AudioFacts {
        container: path
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase()),
        codec,
        bitrate_kbps,
        sample_rate,
        channels: params.channels.map(|channels| channels.count() as u32),
        bits_per_sample: params.bits_per_sample,
        duration_secs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_wav(path: &Path) {
        let data_len = 800u32 * 2;
        let mut file = std::fs::File::create(path).unwrap();
        file.write_all(b"RIFF").unwrap();
        file.write_all(&(36 + data_len).to_le_bytes()).unwrap();
        file.write_all(b"WAVEfmt ").unwrap();
        file.write_all(&16u32.to_le_bytes()).unwrap();
        file.write_all(&1u16.to_le_bytes()).unwrap();
        file.write_all(&1u16.to_le_bytes()).unwrap();
        file.write_all(&8000u32.to_le_bytes()).unwrap();
        file.write_all(&16000u32.to_le_bytes()).unwrap();
        file.write_all(&2u16.to_le_bytes()).unwrap();
        file.write_all(&16u16.to_le_bytes()).unwrap();
        file.write_all(b"data").unwrap();
        file.write_all(&data_len.to_le_bytes()).unwrap();
        file.write_all(&vec![0u8; data_len as usize]).unwrap();
    }

    #[test]
    fn probes_a_wav_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.wav");
        write_wav(&path);

        let facts = probe_audio(&path).unwrap();
        assert_eq!(facts.container.as_deref(), Some("wav"));
        assert_eq!(facts.sample_rate, Some(8000));
        assert_eq!(facts.channels, Some(1));
        assert_eq!(facts.bits_per_sample, Some(16));
        let duration = facts.duration_secs.unwrap();
        assert!((duration - 0.1).abs() < 0.001);
        assert!(facts.bitrate_kbps.unwrap() > 0);
    }

    #[test]
    fn rejects_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(probe_audio(&dir.path().join("missing.wav")).is_err());
    }
}
