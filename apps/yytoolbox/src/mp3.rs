//! Bounded, streaming MP3 output from stereo 48 kHz signed-16 little-endian PCM.
//! Pure Rust: no C encoder, DLL, external executable, or full-recording PCM buffer.
use crate::Result;
use rusty_mp3::{Error, Mp3Encoder, Mp3EncoderConfig};
use std::io::{Seek, SeekFrom, Write};

pub struct Encoder<W> {
    codec: Mp3Encoder,
    writer: W,
    start: u64,
    frames: u64,
}

impl<W: Write + Seek> Encoder<W> {
    pub fn new(mut writer: W) -> Result<Self> {
        let start = writer.stream_position().map_err(|e| e.to_string())?;
        Ok(Self {
            // VBR emits packets incrementally. The library's CBR reservoir path
            // banks all frames until finish and is unsuitable for hour-long capture.
            codec: Mp3Encoder::new(Mp3EncoderConfig {
                bitrate_kbps: 192,
                vbr_quality: Some(rusty_mp3::vbr_quality_index(2.0)),
            }),
            writer,
            start,
            frames: 0,
        })
    }

    pub fn push(&mut self, pcm: &[u8]) -> Result<()> {
        if pcm.len() % 4 != 0 {
            return Err("MP3入力は16bitステレオPCMの完全なフレームが必要です".into());
        }
        // Bound the codec's output queue even if a caller passes a large buffer.
        for block in pcm.chunks(1152 * 4) {
            self.codec
                .push_pcm_s16le(block, 2, 48000)
                .map_err(|e| format!("MP3変換に失敗しました: {e}"))?;
            self.frames += block.len() as u64 / 4;
            self.drain(false)?;
        }
        Ok(())
    }

    fn drain(&mut self, finished: bool) -> Result<()> {
        loop {
            match self.codec.next_packet() {
                Ok(packet) => self
                    .writer
                    .write_all(&packet)
                    .map_err(|e| format!("MP3を書き込めません: {e}"))?,
                Err(Error::Again) if !finished => return Ok(()),
                Err(Error::Eof) if finished => return Ok(()),
                Err(error) => return Err(format!("MP3変換に失敗しました: {error}")),
            }
        }
    }

    pub fn finish(mut self) -> Result<W> {
        if self.frames == 0 {
            return Err("保存する音声がありません".into());
        }
        self.codec.finish();
        self.drain(true)?;
        let tag = self
            .codec
            .lame_tag_frame()
            .ok_or("MP3の再生時間情報を作成できません")?;
        self.writer
            .seek(SeekFrom::Start(self.start))
            .and_then(|_| self.writer.write_all(tag))
            .and_then(|_| self.writer.seek(SeekFrom::End(0)))
            .and_then(|_| self.writer.flush())
            .map_err(|e| format!("MP3を確定できません: {e}"))?;
        Ok(self.writer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    pub(super) fn tone(frames: usize) -> Vec<u8> {
        (0..frames)
            .flat_map(|i| {
                let left =
                    (12000.0 * (i as f64 * std::f64::consts::TAU * 440.0 / 48000.0).sin()) as i16;
                let right =
                    (8000.0 * (i as f64 * std::f64::consts::TAU * 880.0 / 48000.0).sin()) as i16;
                [left.to_le_bytes(), right.to_le_bytes()].concat()
            })
            .collect()
    }

    #[test]
    fn streaming_mp3_roundtrip_preserves_channels_duration_and_signal() {
        let pcm = tone(48000);
        let mut encoder = Encoder::new(Cursor::new(Vec::new())).unwrap();
        for block in pcm.chunks(960 * 4) {
            encoder.push(block).unwrap();
        }
        assert!(
            encoder.writer.get_ref().len() > 1000,
            "must emit before finish"
        );
        let bytes = encoder.finish().unwrap().into_inner();
        assert!(bytes.len() < pcm.len() / 2);
        let mut decoder = rusty_mp3::Mp3Decoder::new();
        decoder.push(&bytes);
        decoder.flush();
        let mut decoded = Vec::new();
        loop {
            match decoder.next_frame() {
                Ok(frame) => {
                    assert_eq!(frame.channels, 2);
                    assert_eq!(frame.sample_rate, 48000);
                    decoded.extend(frame.samples);
                }
                Err(Error::Eof) => break,
                other => panic!("invalid MP3: {other:?}"),
            }
        }
        // The raw decoder includes the tag frame and codec delay. Verify the
        // gapless metadata independently, then compare the presentation samples.
        assert_eq!(&bytes[36..40], b"Xing");
        let count = u32::from_be_bytes(bytes[44..48].try_into().unwrap()) as usize;
        let size = u32::from_be_bytes(bytes[48..52].try_into().unwrap()) as usize;
        let delay = ((usize::from(bytes[177])) << 4) | (usize::from(bytes[178]) >> 4);
        let padding = ((usize::from(bytes[178]) & 15) << 8) | usize::from(bytes[179]);
        assert_eq!(size, bytes.len());
        assert_eq!(count * 1152 - delay - padding, 48000);
        assert_eq!(decoded.len(), (count + 1) * 1152 * 2);
        let offset = (1152 + delay + rusty_mp3::encode::lametag::DECODER_DELAY) * 2;
        let decoded = &decoded[offset..offset + 48000 * 2];
        for channel in 0..2 {
            let original: Vec<f64> = pcm
                .chunks_exact(4)
                .map(|p| {
                    f64::from(i16::from_le_bytes([p[channel * 2], p[channel * 2 + 1]])) / 32768.0
                })
                .collect();
            let samples: Vec<f64> = decoded
                .iter()
                .skip(channel)
                .step_by(2)
                .map(|v| f64::from(*v))
                .collect();
            let signal: f64 = original.iter().map(|v| v * v).sum();
            let error: f64 = original
                .iter()
                .zip(&samples)
                .map(|(a, b)| (a - b).powi(2))
                .sum();
            assert!(
                10.0 * (signal / error).log10() > 18.0,
                "channel {channel}: poor SNR"
            );
        }
    }

    #[test]
    fn short_silent_and_full_scale_input_flush_and_reject_partial_pcm() {
        for frames in [1, 31, 1151, 1152, 1153] {
            let mut encoder = Encoder::new(Cursor::new(Vec::new())).unwrap();
            assert!(encoder.push(&[1, 2, 3]).is_err());
            encoder.push(&[0, 0, 0, 0].repeat(frames)).unwrap();
            let bytes = encoder.finish().unwrap().into_inner();
            assert!(bytes.len() > 100);
        }
        let mut encoder = Encoder::new(Cursor::new(Vec::new())).unwrap();
        encoder.push(&[255, 127, 0, 128].repeat(2400)).unwrap();
        assert!(!encoder.finish().unwrap().into_inner().is_empty());
        assert!(
            Encoder::new(Cursor::new(Vec::new()))
                .unwrap()
                .finish()
                .is_err()
        );
    }

    #[test]
    fn write_errors_propagate_and_tag_patch_preserves_a_prefix() {
        let mut writer = Cursor::new(b"prefix".to_vec());
        writer.set_position(6);
        let mut encoder = Encoder::new(writer).unwrap();
        encoder.push(&tone(2400)).unwrap();
        assert_eq!(&encoder.finish().unwrap().into_inner()[..6], b"prefix");
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("disk full"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl Seek for Broken {
            fn seek(&mut self, _: SeekFrom) -> std::io::Result<u64> {
                Ok(0)
            }
        }
        assert!(
            Encoder::new(Broken)
                .unwrap()
                .push(&tone(2400))
                .unwrap_err()
                .contains("disk full")
        );
    }
}
