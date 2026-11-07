//! Protocol facts selected in AIRPLAY_REFERENCE_ANALYSIS.md. No sender code port.
use anyhow::{ensure, Context, Result};
use plist::{Dictionary, Value};
use std::io::Cursor;
use zeroize::{Zeroize, Zeroizing};
pub const RATE: u32 = 44_100;
pub const SPF: usize = 352;
pub const FORMAT: u64 = 0x40000;
pub const MIN_LATENCY: u32 = 11025;
pub const MAX_LATENCY: u32 = 88200;
pub fn dict(items: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Dictionary(items.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}
pub fn uint(n: u64) -> Value {
    Value::Integer(n.into())
}
pub fn binary(value: &mut Value) -> Result<Zeroizing<Vec<u8>>> {
    let mut bytes = Zeroizing::new(Vec::new());
    let result = value.to_writer_binary(&mut *bytes);
    fn wipe(v: &mut Value) {
        match v {
            Value::Data(bytes) => bytes.zeroize(),
            Value::Array(a) => a.iter_mut().for_each(wipe),
            Value::Dictionary(d) => d.values_mut().for_each(wipe),
            _ => (),
        }
    }
    wipe(value);
    result?;
    ensure!(bytes.len() <= 8192, "plist request exceeds limit");
    Ok(bytes)
}
/// Bound recursive expansion before asking the general plist decoder to allocate.
pub fn parse(bytes: &[u8]) -> Result<Value> {
    ensure!(
        bytes.len() >= 40 && bytes.len() <= 8192 && bytes.starts_with(b"bplist00"),
        "expected bounded binary plist"
    );
    fn number(b: &[u8]) -> Result<usize> {
        ensure!(!b.is_empty() && b.len() <= 8, "invalid plist integer width");
        let mut n = 0usize;
        for x in b {
            n = n
                .checked_mul(256)
                .and_then(|n| n.checked_add(usize::from(*x)))
                .context("plist integer overflow")?;
        }
        Ok(n)
    }
    let trailer = &bytes[bytes.len() - 32..];
    let offset_size = usize::from(trailer[6]);
    let ref_size = usize::from(trailer[7]);
    ensure!(
        (1..=8).contains(&offset_size) && (1..=8).contains(&ref_size),
        "invalid plist widths"
    );
    let objects = number(&trailer[8..16])?;
    let root = number(&trailer[16..24])?;
    let table = number(&trailer[24..32])?;
    ensure!(
        objects > 0
            && objects <= 2048
            && root < objects
            && table >= 8
            && table
                .checked_add(objects * offset_size)
                .is_some_and(|end| end <= bytes.len() - 32),
        "invalid plist table"
    );
    let data = &bytes[..table];
    fn visit(
        data: &[u8],
        offsets: &[usize],
        refs: usize,
        id: usize,
        depth: usize,
        budget: &mut usize,
    ) -> Result<()> {
        ensure!(depth <= 24 && *budget > 0, "plist expansion limit");
        *budget -= 1;
        let pos = *offsets.get(id).context("invalid plist reference")?;
        let tag = *data.get(pos).context("invalid plist object")?;
        let kind = tag >> 4;
        let mut count = usize::from(tag & 15);
        let mut start = pos + 1;
        if [4, 5, 6, 10, 13].contains(&kind) && count == 15 {
            let size_tag = *data.get(start).context("plist length missing")?;
            ensure!(
                size_tag >> 4 == 1 && size_tag & 15 <= 3,
                "invalid plist extended length"
            );
            let width = 1usize << (size_tag & 15);
            start += 1;
            count = number(
                data.get(start..start + width)
                    .context("plist length truncated")?,
            )?;
            start += width;
        }
        match kind {
            0 => ensure!(
                [0, 8, 9].contains(&(tag & 15)),
                "unsupported plist simple value"
            ),
            1 | 2 => {
                ensure!(tag & 15 <= 3, "unsupported plist numeric size");
                ensure!(
                    data.get(start..start + (1usize << (tag & 15))).is_some(),
                    "truncated plist number"
                );
            }
            3 => {
                ensure!(
                    tag == 0x33 && data.get(start..start + 8).is_some(),
                    "invalid plist date"
                );
            }
            4..=6 => {
                ensure!(
                    count <= 8192
                        && data
                            .get(start..start + count * if kind == 6 { 2 } else { 1 })
                            .is_some(),
                    "invalid plist scalar length"
                );
            }
            10 | 13 => {
                ensure!(count <= 256, "plist collection too large");
                let entries = count * if kind == 13 { 2 } else { 1 };
                let end = start + entries * refs;
                let list = data.get(start..end).context("truncated plist collection")?;
                for entry in list.chunks(refs) {
                    visit(data, offsets, refs, number(entry)?, depth + 1, budget)?;
                }
            }
            _ => anyhow::bail!("unsupported plist object type"),
        }
        Ok(())
    }
    let offsets: Vec<_> = bytes[table..table + objects * offset_size]
        .chunks(offset_size)
        .map(number)
        .collect::<Result<_>>()?;
    ensure!(
        offsets.iter().all(|p| *p >= 8 && *p < table),
        "plist offset out of range"
    );
    visit(data, &offsets, ref_size, root, 0, &mut 2048)?;
    Value::from_reader(Cursor::new(bytes)).context("invalid binary plist")
}
pub fn dictionary(value: &Value) -> Result<&Dictionary> {
    value.as_dictionary().context("expected plist dictionary")
}
pub fn integer(d: &Dictionary, key: &str) -> Result<u64> {
    d.get(key)
        .and_then(Value::as_unsigned_integer)
        .with_context(|| format!("missing or invalid unsigned plist field {key}"))
}
pub fn port(d: &Dictionary, key: &str) -> Result<u16> {
    let p = u16::try_from(integer(d, key)?).with_context(|| format!("invalid negotiated {key}"))?;
    ensure!(p != 0, "zero negotiated {key}");
    Ok(p)
}
pub fn capabilities(value: &Value) -> Result<()> {
    let d = dictionary(value)?;
    let f = integer(d, "features")?;
    ensure!(
        f & (1 << 9) != 0 && f & (1 << 19) != 0,
        "receiver does not advertise audio and ALAC; no codec fallback"
    );
    if let Some(flags) = d.get("statusFlags") {
        ensure!(
            flags
                .as_unsigned_integer()
                .is_some_and(|f| f & (1 << 7) == 0),
            "password/FairPlay/MFi authorization unsupported"
        );
    }
    if let Some(formats) = d.get("supportedFormats") {
        let formats = dictionary(formats)?;
        ensure!(
            integer(formats, "audioStream")? & FORMAT != 0,
            "receiver excludes realtime ALAC/44100/16/2"
        );
    }
    Ok(())
}
pub fn stream_offer(key: &[u8; 32], control_port: u16, session: u32) -> Result<Zeroizing<Vec<u8>>> {
    binary(&mut dict([(
        "streams",
        Value::Array(vec![dict([
            ("audioFormat", uint(FORMAT)),
            ("audioMode", Value::String("default".into())),
            ("ct", uint(2)),
            ("spf", uint(SPF as u64)),
            ("sr", uint(u64::from(RATE))),
            ("type", uint(96)),
            ("isMedia", Value::Boolean(true)),
            ("latencyMin", uint(u64::from(MIN_LATENCY))),
            ("latencyMax", uint(u64::from(MAX_LATENCY))),
            ("controlPort", uint(u64::from(control_port))),
            ("shk", Value::Data(key.to_vec())),
            ("streamConnectionID", uint(u64::from(session))),
            ("supportsDynamicStreamID", Value::Boolean(false)),
        ])]),
    )]))
}
pub struct Stream {
    pub data: u16,
    pub control: u16,
    pub id: Option<u32>,
}
pub fn stream(value: &Value) -> Result<Stream> {
    let d = dictionary(value)?;
    let streams = d
        .get("streams")
        .and_then(Value::as_array)
        .context("streams absent")?;
    ensure!(streams.len() == 1, "expected exactly one stream");
    let s = dictionary(&streams[0])?;
    ensure!(
        integer(s, "type")? == 96,
        "unsupported negotiated stream type"
    );
    for (field, expected) in [
        ("audioFormat", FORMAT),
        ("ct", 2),
        ("sr", u64::from(RATE)),
        ("spf", SPF as u64),
        ("latencyMin", u64::from(MIN_LATENCY)),
        ("latencyMax", u64::from(MAX_LATENCY)),
        ("et", 32),
    ] {
        if s.contains_key(field) {
            ensure!(
                integer(s, field)? == expected,
                "receiver changed requested codec parameters"
            );
        }
    }
    if s.contains_key("streamID") {
        ensure!(
            integer(s, "streamID")? <= u64::from(u32::MAX),
            "invalid stream identifier"
        );
    }
    let id = if s.contains_key("streamID") {
        Some(u32::try_from(integer(s, "streamID")?)?)
    } else {
        None
    };
    Ok(Stream {
        data: port(s, "dataPort")?,
        control: port(s, "controlPort")?,
        id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn offer_matches_independently_serialized_protocol_fixture() {
        let key: [u8; 32] = (0..32).collect::<Vec<_>>().try_into().unwrap();
        let offer = stream_offer(&key, 6001, 1).unwrap();
        assert_eq!(
            parse(&offer).unwrap(),
            parse(include_bytes!("../../tests/fixtures/realtime-offer.plist")).unwrap()
        );
        assert!(parse(include_bytes!("../../tests/fixtures/cyclic.plist")).is_err());
        assert!(parse(b"<plist></plist>").is_err());
        assert!(parse(&[0; 8193]).is_err());
    }
    #[test]
    fn reject_incompatible_formats_and_invalid_ports() {
        let good = dict([("features", uint((1 << 9) | (1 << 19)))]);
        assert!(capabilities(&good).is_ok());
        assert!(capabilities(&dict([("features", uint(1 << 9))])).is_err());
        for (field, value) in [
            ("type", uint(103)),
            ("dataPort", uint(65536)),
            ("controlPort", uint(0)),
            ("ct", uint(1)),
            ("sr", uint(48000)),
            ("spf", uint(1024)),
            ("streamID", uint(u64::MAX)),
        ] {
            let mut s = dict([
                ("type", uint(96)),
                ("dataPort", uint(6000)),
                ("controlPort", uint(6001)),
            ]);
            s.as_dictionary_mut().unwrap().insert(field.into(), value);
            assert!(stream(&dict([("streams", Value::Array(vec![s]))])).is_err());
        }
    }
}
