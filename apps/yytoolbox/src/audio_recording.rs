//! WASAPI audio-only recording with a Pure Rust streaming MP3 encoder.
use super::*;

const RATE: u64 = 48_000;
const BLOCK: u64 = 4; // Stereo, signed 16-bit little endian.

pub(super) fn record(plan: Plan, cancelled: &AtomicBool) -> Result<Output> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("録音開始前に中止しました".into());
    }
    let begin = Arc::new(AtomicBool::new(false));
    let mut sources = Vec::new();
    if plan.system {
        sources.push(Audio::start(true, begin.clone())?);
    }
    if plan.microphone {
        sources.push(Audio::start(false, begin.clone())?);
    }
    begin.store(true, Ordering::Release);
    record_pcm(&plan, cancelled, |frames| mix(&sources, frames))
}

fn mix(sources: &[Audio], frames: usize) -> Result<Vec<u8>> {
    if sources.is_empty() {
        return Err("録音する音声デバイスがありません".into());
    }
    let packets = sources
        .iter()
        .map(|source| source.take(frames * 2))
        .collect::<Result<Vec<_>>>()?;
    Ok((0..frames * 2)
        .flat_map(|i| {
            let sum: i32 = packets.iter().map(|packet| i32::from(packet[i])).sum();
            ((sum / packets.len() as i32) as i16).to_le_bytes()
        })
        .collect())
}

fn record_pcm(
    plan: &Plan,
    cancelled: &AtomicBool,
    mut pcm: impl FnMut(usize) -> Result<Vec<u8>>,
) -> Result<Output> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("録音開始前に中止しました".into());
    }
    let temporary = tempfile::Builder::new()
        .suffix(".mp3")
        .tempfile_in(
            plan.destination
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(std::path::Path::new(".")),
        )
        .map_err(|e| e.to_string())?;
    let writer = temporary.reopen().map_err(|e| e.to_string())?;
    let mut encoder = yytoolbox::mp3::Encoder::new(writer)?;
    let start = Instant::now();
    let duration = Duration::from_secs(u64::from(plan.seconds));
    let mut frames = 0u64;
    loop {
        let elapsed = start.elapsed().min(duration);
        let stopping = cancelled.load(Ordering::Relaxed) || elapsed == duration;
        let target = ((elapsed.as_nanos() * u128::from(RATE) / 1_000_000_000) as u64)
            .max(u64::from(stopping));
        while frames < target {
            let count = (target - frames).min(960) as usize;
            let data = pcm(count)?;
            if data.len() != count * BLOCK as usize {
                return Err("録音データの長さが不正です".into());
            }
            encoder.push(&data)?;
            frames += count as u64;
        }
        if stopping {
            break;
        }
        std::thread::park_timeout(
            Duration::from_millis(20).min(duration.saturating_sub(start.elapsed())),
        );
    }
    let writer = encoder.finish()?;
    writer
        .sync_all()
        .map_err(|e| format!("MP3を確定できません: {e}"))?;
    drop(writer);
    temporary
        .persist(&plan.destination)
        .map_err(|e| e.error.to_string())?;
    let text = format!(
        "録音を保存しました: {}\nMP3 / VBR品質2 / 48000Hz / ステレオ / {:.2}秒\nPC再生音: {} / マイク: {}",
        plan.destination.display(),
        frames as f64 / RATE as f64,
        plan.system,
        plan.microphone
    );
    Ok(Output {
        bytes: text.as_bytes().to_vec(),
        text,
        filename: "recording-info.txt",
        notice: format!(
            "MP3保存完了: {}。結果欄は録音情報です",
            plan.destination.display()
        ),
        qr: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(path: PathBuf) -> Plan {
        let mut params = yytoolbox::Tool::Recording.defaults();
        params.insert("capture_mode".into(), "音声のみ（MP3）".into());
        params.insert("system_audio".into(), "あり".into());
        params.insert("seconds".into(), "1".into());
        // Audio-only must not depend on valid video settings or a screen rectangle.
        params.insert("fps".into(), "invalid".into());
        params.insert("max_width".into(), "invalid".into());
        Plan::new(path, RECT::default(), &params).unwrap()
    }

    #[test]
    fn mp3_recording_is_decodable_by_windows_without_video() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("音声.mp3");
        let plan = plan(path.clone());
        let mut position = 0;
        let result = record_pcm(&plan, &AtomicBool::new(false), |count| {
            let bytes = (position..position + count)
                .flat_map(|i| {
                    let sample = (12000.0
                        * (i as f64 * std::f64::consts::TAU * 440.0 / 48000.0).sin())
                        as i16;
                    [sample.to_le_bytes(), sample.to_le_bytes()].concat()
                })
                .collect();
            position += count;
            Ok(bytes)
        })
        .unwrap();
        assert!(result.notice.contains("MP3保存完了"));
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(position, 48000);
        assert!(bytes.len() > 1000 && bytes.len() < 48000 * 2);
        assert_ne!(&bytes[..4], b"RIFF");
        let _com = Com::new().unwrap();
        let _media = Media::new().unwrap();
        unsafe {
            let reader =
                MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), None).unwrap();
            let media = reader.GetNativeMediaType(0, 0).unwrap();
            assert_eq!(media.GetGUID(&MF_MT_MAJOR_TYPE).unwrap(), MFMediaType_Audio);
            assert!(
                reader.GetNativeMediaType(1, 0).is_err(),
                "audio-only file must contain one audio stream"
            );
            let output = MFCreateMediaType().unwrap();
            output
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
                .unwrap();
            output.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM).unwrap();
            reader.SetCurrentMediaType(0, None, &output).unwrap();
            let mut samples = 0;
            let mut power = 0.0;
            loop {
                let mut sample = None;
                let mut flags = 0;
                let mut time = 0;
                reader
                    .ReadSample(
                        0,
                        0,
                        None,
                        Some(&mut flags),
                        Some(&mut time),
                        Some(&mut sample),
                    )
                    .unwrap();
                if let Some(sample) = sample {
                    let buffer = sample.ConvertToContiguousBuffer().unwrap();
                    let mut data = std::ptr::null_mut();
                    let mut len = 0;
                    buffer.Lock(&mut data, None, Some(&mut len)).unwrap();
                    for pair in std::slice::from_raw_parts(data, len as usize).chunks_exact(2) {
                        power += f64::from(i16::from_le_bytes([pair[0], pair[1]])).powi(2);
                        samples += 1;
                    }
                    buffer.Unlock().unwrap();
                }
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    break;
                }
                assert!(samples < 53000 * 2);
            }
            assert!((48000 * 2..=53000 * 2).contains(&samples));
            assert!(
                (power / samples as f64).sqrt() > 1000.0,
                "Windows decoder must recover the tone"
            );
        }
    }

    #[test]
    fn early_stop_saves_short_mp3_and_errors_preserve_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("short.mp3");
        let mut plan = plan(path.clone());
        plan.seconds = 60;
        let stop = AtomicBool::new(false);
        record_pcm(&plan, &stop, |count| {
            stop.store(true, Ordering::Relaxed);
            Ok(vec![0; count * 4])
        })
        .unwrap();
        let saved = std::fs::read(&path).unwrap();
        assert!(saved.len() > 100 && saved.len() < 48000 * 4);
        assert!(
            record_pcm(&plan, &AtomicBool::new(true), |_| panic!(
                "must not capture"
            ))
            .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), saved);
        assert!(
            record_pcm(&plan, &AtomicBool::new(false), |_| Err(
                "device disconnected".into()
            ))
            .unwrap_err()
            .contains("device disconnected")
        );
        assert_eq!(std::fs::read(&path).unwrap(), saved);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn audio_only_requires_a_source_and_mp3_extension_and_mixes_without_overflow() {
        let mut params = yytoolbox::Tool::Recording.defaults();
        params.insert("capture_mode".into(), "音声のみ（MP3）".into());
        assert!(
            Plan::new("a.mp3".into(), RECT::default(), &params)
                .err()
                .unwrap()
                .contains("PC再生音")
        );
        params.insert("microphone".into(), "あり".into());
        assert!(Plan::new("a.wav".into(), RECT::default(), &params).is_err());
        assert!(Plan::new("a.MP3".into(), RECT::default(), &params).is_ok());
        let source = |samples| Audio {
            queue: Arc::new(Mutex::new(VecDeque::from(samples))),
            stop: Arc::new(AtomicBool::new(false)),
            error: Arc::new(Mutex::new(None)),
            thread: None,
        };
        let sources = [
            source(vec![i16::MAX, i16::MIN]),
            source(vec![i16::MAX, i16::MIN]),
        ];
        assert_eq!(
            mix(&sources, 1).unwrap(),
            [i16::MAX.to_le_bytes(), i16::MIN.to_le_bytes()].concat()
        );
        assert_eq!(mix(&sources, 1).unwrap(), vec![0; 4]);
        assert!(mix(&[], 1).is_err());
    }
}
