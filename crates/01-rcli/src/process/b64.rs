use crate::Base64Format;
use anyhow::Result;
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use std::io::Read;

pub fn process_encode(reader: &mut dyn Read, format: Base64Format) -> Result<String> {
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf)?;
    let encoded = match format {
        Base64Format::Standard => STANDARD.encode(&buf),
        Base64Format::UrlSafe => URL_SAFE_NO_PAD.encode(&buf),
    };

    Ok(encoded)
}

pub fn process_decode(reader: &mut dyn Read, format: Base64Format) -> Result<String> {
    let mut buf = String::new();
    reader.read_to_string(&mut buf)?;
    // avoid accidental newlines
    let buf = buf.trim();

    let decoded = match format {
        Base64Format::Standard => STANDARD.decode(buf)?,
        Base64Format::UrlSafe => URL_SAFE_NO_PAD.decode(buf)?,
    };
    // TODO: decoded data might not be string (but for this example, we assume it is)
    Ok(String::from_utf8(decoded)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::get_reader;

    #[test]
    fn test_process_encode() -> Result<()> {
        let input = "Cargo.toml";
        let mut reader = get_reader(input)?;
        let format = Base64Format::Standard;
        assert!(process_encode(&mut reader, format).is_ok());
        Ok(())
    }

    #[test]
    fn test_process_decode() -> Result<()> {
        let input = "fixtures/b64.txt";
        let mut reader = get_reader(input)?;
        let format = Base64Format::UrlSafe;
        process_decode(&mut reader, format)?;

        Ok(())
    }

    /// Input: UTF-8 plaintext in memory — standard Base64 encode then decode roundtrip.
    #[test]
    fn test_process_encode_decode_roundtrip_standard() -> Result<()> {
        let plaintext = "hello, 世界";
        let mut enc_in = std::io::Cursor::new(plaintext.as_bytes().to_vec());
        let encoded = process_encode(&mut enc_in, Base64Format::Standard)?;
        let mut dec_in = std::io::Cursor::new(encoded.into_bytes());
        let decoded = process_decode(&mut dec_in, Base64Format::Standard)?;
        assert_eq!(decoded, plaintext);
        Ok(())
    }
}
