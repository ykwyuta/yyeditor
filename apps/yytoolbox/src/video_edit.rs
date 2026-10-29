//! Bounded-memory local video editing through Windows Media Foundation.
use super::*;
use std::path::Path;
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToUInt64};
use yytoolbox::video::{Edit, SECOND, Segment};

// MF's URL resolver misinterprets verbatim Win32 paths as network URLs.
fn media_path(path: &Path) -> HSTRING {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    let wide: Vec<_> = path.as_os_str().encode_wide().collect();
    let prefix: Vec<_> = "\\\\?\\UNC\\".encode_utf16().collect();
    let verbatim: Vec<_> = "\\\\?\\".encode_utf16().collect();
    let normalized = if wide.starts_with(&prefix) {
        [vec![92, 92], wide[prefix.len()..].to_vec()].concat()
    } else if wide.starts_with(&verbatim) {
        wide[verbatim.len()..].to_vec()
    } else {
        wide
    };
    HSTRING::from(std::ffi::OsString::from_wide(&normalized).as_os_str())
}

fn err(error: windows::core::Error) -> String {
    format!("Windowsの動画処理に失敗しました（コーデック・形式も確認してください）: {error}")
}
fn cancelled(flag: &AtomicBool) -> Result<()> {
    if flag.load(Ordering::Relaxed) {
        Err("動画編集を中止しました。出力は保存していません".into())
    } else {
        Ok(())
    }
}
struct Source {
    reader: IMFSourceReader,
    video: u32,
    audio: Option<u32>,
    width: u32,
    height: u32,
    stride: i32,
    rate: u64,
    duration: i64,
}
impl Source {
    fn open(path: &Path) -> Result<Self> {
        unsafe {
            let mut attributes = None;
            MFCreateAttributes(&mut attributes, 1).map_err(err)?;
            let attributes = attributes.unwrap();
            attributes
                .SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)
                .map_err(err)?;
            let reader =
                MFCreateSourceReaderFromURL(&media_path(path), &attributes).map_err(err)?;
            reader
                .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
                .map_err(err)?;
            let (mut video, mut audio) = (None, None);
            for index in 0..128 {
                let native = match reader.GetNativeMediaType(index, 0) {
                    Ok(media) => media,
                    Err(e) if e.code() == MF_E_INVALIDSTREAMNUMBER => break,
                    Err(e) => return Err(err(e)),
                };
                match native.GetGUID(&MF_MT_MAJOR_TYPE).map_err(err)? {
                    major if major == MFMediaType_Video && video.is_none() => video = Some(index),
                    major if major == MFMediaType_Audio && audio.is_none() => audio = Some(index),
                    _ => {}
                }
            }
            let video = video.ok_or("動画の映像トラックがありません")?;
            let native = reader.GetNativeMediaType(video, 0).map_err(err)?;
            if native.GetUINT32(&MF_MT_VIDEO_ROTATION).unwrap_or(0) != 0 {
                return Err(
                    "回転情報付き動画にはまだ対応していません。向きを適用してから編集してください"
                        .into(),
                );
            }
            let aspect = native
                .GetUINT64(&MF_MT_PIXEL_ASPECT_RATIO)
                .unwrap_or((1u64 << 32) | 1);
            if (aspect >> 32) != (aspect & 0xffff_ffff) {
                return Err("正方画素の動画を指定してください".into());
            }
            let output = MFCreateMediaType().map_err(err)?;
            output
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(err)?;
            output
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
                .map_err(err)?;
            reader.SetStreamSelection(video, true).map_err(err)?;
            reader
                .SetCurrentMediaType(video, None, &output)
                .map_err(err)?;
            let output = reader.GetCurrentMediaType(video).map_err(err)?;
            let size = output.GetUINT64(&MF_MT_FRAME_SIZE).map_err(err)?;
            let (width, height) = ((size >> 32) as u32, size as u32);
            if width < 2
                || height < 2
                || width % 2 != 0
                || height % 2 != 0
                || u64::from(width) * u64::from(height) > 32_000_000
            {
                return Err("動画は縦横とも偶数、2px以上、3200万画素以内にしてください".into());
            }
            let rate = output.GetUINT64(&MF_MT_FRAME_RATE).map_err(err)?;
            let (n, d) = (rate >> 32, rate as u32 as u64);
            if n == 0 || d == 0 || n > 240 * d {
                return Err("対応するフレームレートは0超〜240fpsです".into());
            }
            let stride = output
                .GetUINT32(&MF_MT_DEFAULT_STRIDE)
                .map(|n| n as i32)
                .unwrap_or(
                    MFGetStrideForBitmapInfoHeader(MFVideoFormat_RGB32.data1, width)
                        .map_err(err)?,
                );
            if stride.unsigned_abs() < width * 4 {
                return Err("動画の画素配置が不正です".into());
            }
            if let Some(audio) = audio {
                let output = MFCreateMediaType().map_err(err)?;
                output
                    .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
                    .map_err(err)?;
                output
                    .SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)
                    .map_err(err)?;
                output
                    .SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)
                    .map_err(err)?;
                output
                    .SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, 48000)
                    .map_err(err)?;
                output
                    .SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
                    .map_err(err)?;
                reader.SetStreamSelection(audio, true).map_err(err)?;
                reader
                    .SetCurrentMediaType(audio, None, &output)
                    .map_err(err)?;
            }
            let mut duration = reader
                .GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)
                .map_err(err)?;
            let ticks = PropVariantToUInt64(&duration);
            let _ = PropVariantClear(&mut duration);
            let duration =
                i64::try_from(ticks.map_err(err)?).map_err(|_| "動画の長さが上限を超えています")?;
            Ok(Self {
                reader,
                video,
                audio,
                width,
                height,
                stride,
                rate,
                duration,
            })
        }
    }
}

fn bytes(sample: &IMFSample) -> Result<Vec<u8>> {
    unsafe {
        let buffer = sample.ConvertToContiguousBuffer().map_err(err)?;
        let length = buffer.GetCurrentLength().map_err(err)?;
        if length == 0 {
            return Ok(Vec::new());
        }
        if length > 128_000_000 {
            return Err("動画サンプルが大きすぎます".into());
        }
        let mut pointer = std::ptr::null_mut();
        buffer.Lock(&mut pointer, None, None).map_err(err)?;
        let bytes = std::slice::from_raw_parts(pointer, length as usize).to_vec();
        buffer.Unlock().map_err(err)?;
        Ok(bytes)
    }
}
fn overlaps(
    segments: &[Segment],
    start: i64,
    end: i64,
) -> impl Iterator<Item = (i64, i64, i64)> + '_ {
    segments.iter().filter_map(move |s| {
        let (a, b) = (start.max(s.start), end.min(s.end));
        (a < b).then(|| (a, b, s.output + (a - s.start)))
    })
}

pub fn edit(source: &Path, destination: &Path, edit: &Edit, flag: &AtomicBool) -> Result<Output> {
    cancelled(flag)?;
    if destination.exists() {
        return Err("元動画と既存ファイルは上書きしません。新しい保存名を選んでください".into());
    }
    if !destination
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("mp4"))
    {
        return Err("保存先の拡張子は.mp4にしてください".into());
    }
    let source = source.canonicalize().map_err(|e| e.to_string())?;
    if !source.is_file() {
        return Err("ローカルの動画ファイルを指定してください".into());
    }
    let _com = Com::new()?;
    let _media = Media::new()?;
    let input = Source::open(&source)?;
    let segments = edit.segments(input.duration)?;
    let temp = tempfile::Builder::new()
        .suffix(".mp4")
        .tempfile_in(destination.parent().unwrap_or(Path::new(".")))
        .map_err(|e| e.to_string())?
        .into_temp_path();
    let encoder = Encoder::with_rate(
        &temp,
        input.width,
        input.height,
        input.rate,
        input.audio.is_some(),
    )?;
    let frame_duration = (SECOND as u64 * (input.rate as u32 as u64) / (input.rate >> 32)) as i64;
    let mut frames = 0u64;
    let mut active = if input.audio.is_some() { 2 } else { 1 };
    let mut frame = vec![0; input.width as usize * input.height as usize * 4];
    while active > 0 {
        cancelled(flag)?;
        let (mut stream, mut flags, mut time, mut sample) = (0, 0, 0, None);
        unsafe {
            input.reader.ReadSample(
                MF_SOURCE_READER_ANY_STREAM.0 as u32,
                0,
                Some(&mut stream),
                Some(&mut flags),
                Some(&mut time),
                Some(&mut sample),
            )
        }
        .map_err(err)?;
        if flags
            & (MF_SOURCE_READERF_ERROR.0
                | MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0
                | MF_SOURCE_READERF_NATIVEMEDIATYPECHANGED.0) as u32
            != 0
        {
            return Err("動画の途中で形式が変わったか、デコードに失敗しました".into());
        }
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            unsafe { input.reader.SetStreamSelection(stream, false) }.map_err(err)?;
            active -= 1;
        }
        let Some(sample) = sample else {
            continue;
        };
        let data = bytes(&sample)?;
        if stream == input.video {
            let duration = unsafe { sample.GetSampleDuration() }.unwrap_or(frame_duration);
            if duration <= 0 {
                return Err("映像フレームの長さが不正です".into());
            }
            let end = time
                .checked_add(duration)
                .ok_or("映像の時刻が上限を超えています")?;
            let stride = input.stride.unsigned_abs() as usize;
            if data.len() < stride * input.height as usize {
                return Err("映像データが不足しています".into());
            }
            for y in 0..input.height as usize {
                let row = if input.stride < 0 {
                    input.height as usize - 1 - y
                } else {
                    y
                };
                let width = input.width as usize * 4;
                frame[y * width..(y + 1) * width]
                    .copy_from_slice(&data[row * stride..row * stride + width]);
            }
            for (a, b, output) in overlaps(&segments, time, end) {
                encoder.sample(encoder.video, &frame, output, b - a)?;
                frames += 1;
            }
        } else if Some(stream) == input.audio {
            let count = data.len() / 4;
            let end = time
                .checked_add(count as i64 * SECOND / 48000)
                .ok_or("音声の時刻が上限を超えています")?;
            for (a, b, output) in overlaps(&segments, time, end) {
                let first = (((a - time) as i128 * 48000 + i128::from(SECOND - 1))
                    / i128::from(SECOND)) as usize;
                let last = (((b - time) as i128 * 48000) / i128::from(SECOND)) as usize;
                if first < last && last <= count {
                    encoder.sample(
                        encoder.audio.unwrap(),
                        &data[first * 4..last * 4],
                        output + first as i64 * SECOND / 48000 - (a - time),
                        (last - first) as i64 * SECOND / 48000,
                    )?;
                }
            }
        }
    }
    if frames == 0 {
        return Err("指定範囲に映像がありません".into());
    }
    cancelled(flag)?;
    encoder.finish()?;
    drop(encoder);
    cancelled(flag)?;
    temp.persist_noclobber(destination)
        .map_err(|e| e.error.to_string())?;
    let last = segments.last().unwrap();
    let seconds = (last.output + last.end - last.start) as f64 / SECOND as f64;
    let text = format!(
        "編集した動画を保存しました: {}\n{:.3}秒 / {}×{}\n映像:H.264 / 音声:{}\n元動画: {}",
        destination.display(),
        seconds,
        input.width,
        input.height,
        if input.audio.is_some() {
            "AAC（先頭トラック）"
        } else {
            "なし"
        },
        source.display()
    );
    Ok(Output {
        bytes: text.as_bytes().to_vec(),
        text,
        filename: "video-edit-info.txt",
        notice: "動画のMP4保存が完了しました。結果欄は編集情報です".into(),
        qr: None,
    })
}

#[cfg(test)]
#[path = "video_edit_tests.rs"]
mod tests;
