use super::*;
use yytoolbox::{Params, Tool};

fn fixture(path: &Path, audio: bool) {
    let encoder = Encoder::new(path, 64, 48, 10, audio).unwrap();
    for index in 0..30 {
        let color = match index / 10 {
            0 => [0, 0, 240, 255],
            1 => [0, 240, 0, 255],
            _ => [240, 0, 0, 255],
        };
        let mut pixels = vec![0; 64 * 48 * 4];
        for y in 0..48 {
            for x in 0..64 {
                pixels[(y * 64 + x) * 4..(y * 64 + x + 1) * 4].copy_from_slice(if y < 24 {
                    &color
                } else {
                    &[235, 235, 235, 255]
                });
            }
        }
        encoder
            .sample(
                encoder.video,
                &pixels,
                i64::from(index) * SECOND / 10,
                SECOND / 10,
            )
            .unwrap();
        if let Some(stream) = encoder.audio {
            let mut samples = Vec::new();
            let frequency = match index / 10 {
                0 => 440.0,
                1 => 880.0,
                _ => 1320.0,
            };
            for n in 0..4800 {
                let value = (((index * 4800 + n) as f64 * frequency * std::f64::consts::TAU
                    / 48000.0)
                    .sin()
                    * 8000.0) as i16;
                samples.extend_from_slice(&value.to_le_bytes());
                samples.extend_from_slice(&value.to_le_bytes());
            }
            encoder
                .sample(
                    stream,
                    &samples,
                    i64::from(index) * SECOND / 10,
                    SECOND / 10,
                )
                .unwrap();
        }
    }
    encoder.finish().unwrap();
}

struct Decoded {
    colors: Vec<usize>,
    times: Vec<i64>,
    audio: Vec<(i64, Vec<i16>)>,
    duration: i64,
}
fn decoded(path: &Path) -> Decoded {
    let source = Source::open(path).unwrap();
    let mut result = Decoded {
        colors: Vec::new(),
        times: Vec::new(),
        audio: Vec::new(),
        duration: source.duration,
    };
    let mut active = if source.audio.is_some() { 2 } else { 1 };
    while active > 0 {
        let (mut stream, mut flags, mut time, mut sample) = (0, 0, 0, None);
        unsafe {
            source
                .reader
                .ReadSample(
                    MF_SOURCE_READER_ANY_STREAM.0 as u32,
                    0,
                    Some(&mut stream),
                    Some(&mut flags),
                    Some(&mut time),
                    Some(&mut sample),
                )
                .unwrap();
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                source.reader.SetStreamSelection(stream, false).unwrap();
                active -= 1;
            }
        }
        if let Some(sample) = sample {
            let data = bytes(&sample).unwrap();
            if stream == source.video {
                let stride = source.stride.unsigned_abs() as usize;
                let row = |y: usize| {
                    if source.stride < 0 {
                        source.height as usize - 1 - y
                    } else {
                        y
                    }
                };
                let top = &data[row(10) * stride + 10 * 4..row(10) * stride + 10 * 4 + 3];
                let bottom = &data[row(38) * stride + 10 * 4..row(38) * stride + 10 * 4 + 3];
                assert!(
                    bottom.iter().all(|v| *v > 190),
                    "bottom changed orientation: {bottom:?}, top={top:?}"
                );
                let color = top.iter().enumerate().max_by_key(|(_, v)| **v).unwrap().0;
                assert!(
                    top[color] > 170 && top.iter().enumerate().all(|(i, v)| i == color || *v < 70),
                    "unexpected scene: {top:?}"
                );
                result.colors.push(color);
                result.times.push(time);
            } else {
                result.audio.push((
                    time,
                    data.chunks_exact(4)
                        .map(|b| i16::from_le_bytes([b[0], b[1]]))
                        .collect(),
                ));
            }
        }
    }
    result
}
fn frequency(decoded: &Decoded, start: f64, end: f64) -> f64 {
    let samples: Vec<_> = decoded
        .audio
        .iter()
        .flat_map(|(time, samples)| {
            samples.iter().enumerate().filter_map(move |(i, value)| {
                let t = *time as f64 / SECOND as f64 + i as f64 / 48000.0;
                (start <= t && t < end).then_some(*value)
            })
        })
        .collect();
    assert!(samples.iter().any(|s| s.unsigned_abs() > 1000));
    samples.windows(2).filter(|v| v[0] <= 0 && v[1] > 0).count() as f64 / (end - start)
}
#[test]
fn native_clips_and_cuts_preserve_scenes_orientation_audio_and_continuous_timeline() {
    let _com = Com::new().unwrap();
    let _media = Media::new().unwrap();
    let files = tempfile::tempdir().unwrap();
    let source = files.path().join("元動画 input.mp4");
    fixture(&source, true);
    let original = std::fs::read(&source).unwrap();
    for (mode, ranges, expected, seconds) in [
        (
            "クリップ（残す）",
            "0.5-1.5",
            vec![2, 2, 2, 2, 2, 1, 1, 1, 1, 1],
            1.0,
        ),
        (
            "カット（削除）",
            "1-2",
            [vec![2; 10], vec![0; 10]].concat(),
            2.0,
        ),
        (
            "クリップ（残す）",
            "2-2.5, 0-0.5",
            [vec![2; 5], vec![0; 5]].concat(),
            1.0,
        ),
    ] {
        let output = files
            .path()
            .join(format!("edit-{}.mp4", ranges.replace([',', ' '], "_")));
        let params = Params::from([
            ("edit_mode".into(), mode.into()),
            ("ranges".into(), ranges.into()),
        ]);
        edit(
            &source,
            &output,
            &Edit::parse(&params).unwrap(),
            &AtomicBool::new(false),
        )
        .unwrap();
        let decoded = decoded(&output);
        assert_eq!(decoded.colors, expected, "{mode} {ranges}");
        assert!(decoded.times.first().unwrap().abs() < SECOND / 100);
        assert!(
            decoded
                .times
                .windows(2)
                .all(|v| (v[1] - v[0] - SECOND / 10).abs() < 10)
        );
        assert!((decoded.duration as f64 / SECOND as f64 - seconds).abs() < 0.12);
        assert!((frequency(&decoded, 0.1, 0.4) - 440.0).abs() < 35.0);
        let latter = if mode.starts_with("カット") {
            (1.2, 1.8, 1320.0)
        } else if ranges.starts_with("2-") {
            (0.6, 0.9, 1320.0)
        } else {
            (0.6, 0.9, 880.0)
        };
        assert!((frequency(&decoded, latter.0, latter.1) - latter.2).abs() < 45.0);
    }
    // An edit point may fall inside a frame, rather than on a keyframe or
    // nominal frame boundary. The retained frame durations must close the gap.
    let partial = files.path().join("partial-frame.mp4");
    let params = Params::from([("ranges".into(), "0.05-0.15".into())]);
    edit(
        &source,
        &partial,
        &Edit::parse(&params).unwrap(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let part = decoded(&partial);
    // The H.264 encoder keeps the nominal 10 fps: a 100 ms clip is one frame.
    assert_eq!(part.colors, vec![2]);
    assert_eq!(part.times, vec![0]);
    assert!((part.duration - SECOND / 10).abs() < SECOND / 25);
    assert_eq!(std::fs::read(&source).unwrap(), original);
}
#[test]
fn silent_video_and_failures_do_not_overwrite_inputs_or_leave_partial_output() {
    let _com = Com::new().unwrap();
    let _media = Media::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("silent.mp4");
    fixture(&source, false);
    let mut params = Tool::VideoEdit.defaults();
    params.insert("ranges".into(), "0-1".into());
    let edit_plan = Edit::parse(&params).unwrap();
    let output = dir.path().join("out.mp4");
    assert!(edit(&source, &output, &edit_plan, &AtomicBool::new(true)).is_err());
    assert!(!output.exists());
    assert!(edit(&source, &source, &edit_plan, &AtomicBool::new(false)).is_err());
    params.insert("ranges".into(), "0-10".into());
    assert!(
        edit(
            &source,
            &output,
            &Edit::parse(&params).unwrap(),
            &AtomicBool::new(false)
        )
        .is_err()
    );
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    edit(&source, &output, &edit_plan, &AtomicBool::new(false)).unwrap();
    let result = decoded(&output);
    assert_eq!(result.colors, vec![2; 10]);
    assert!(result.audio.is_empty());
    let before = std::fs::read(&output).unwrap();
    assert!(edit(&source, &output, &edit_plan, &AtomicBool::new(false)).is_err());
    assert_eq!(std::fs::read(output).unwrap(), before);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
}
