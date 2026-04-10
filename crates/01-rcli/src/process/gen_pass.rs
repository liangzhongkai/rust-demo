use rand::seq::SliceRandom;

const UPPER: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ";
const LOWER: &[u8] = b"abcdefghijkmnopqrstuvwxyz";
const NUMBER: &[u8] = b"123456789";
const SYMBOL: &[u8] = b"!@#$%^&*_";

pub fn process_genpass(
    length: u8,
    upper: bool,
    lower: bool,
    number: bool,
    symbol: bool,
) -> anyhow::Result<String> {
    let mut rng = rand::thread_rng();
    let mut password = Vec::new();
    let mut chars = Vec::new();

    if upper {
        chars.extend_from_slice(UPPER);
        password.push(*UPPER.choose(&mut rng).expect("UPPER won't be empty"));
    }
    if lower {
        chars.extend_from_slice(LOWER);
        password.push(*LOWER.choose(&mut rng).expect("LOWER won't be empty"));
    }
    if number {
        chars.extend_from_slice(NUMBER);
        password.push(*NUMBER.choose(&mut rng).expect("NUMBER won't be empty"));
    }
    if symbol {
        chars.extend_from_slice(SYMBOL);
        password.push(*SYMBOL.choose(&mut rng).expect("SYMBOL won't be empty"));
    }

    for _ in 0..(length - password.len() as u8) {
        let c = chars
            .choose(&mut rng)
            .expect("chars won't be empty in this context");
        password.push(*c);
    }

    password.shuffle(&mut rng);

    Ok(String::from_utf8(password)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contains_any(pwd: &str, alphabet: &[u8]) -> bool {
        pwd.bytes().any(|b| alphabet.contains(&b))
    }

    /// Input: length 16, all character classes enabled — output length and coverage.
    #[test]
    fn genpass_input_all_classes_length_16() {
        let pwd = process_genpass(16, true, true, true, true).expect("valid opts");
        assert_eq!(pwd.len(), 16);
        assert!(contains_any(&pwd, UPPER));
        assert!(contains_any(&pwd, LOWER));
        assert!(contains_any(&pwd, NUMBER));
        assert!(contains_any(&pwd, SYMBOL));
    }

    /// Input: no symbols — password must not contain symbol alphabet.
    #[test]
    fn genpass_input_without_symbol() {
        for _ in 0..32 {
            let pwd = process_genpass(24, true, true, true, false).expect("valid opts");
            assert_eq!(pwd.len(), 24);
            assert!(
                !pwd.bytes().any(|b| SYMBOL.contains(&b)),
                "unexpected symbol in: {pwd}"
            );
        }
    }

    /// Input: numbers only — every byte is from NUMBER.
    #[test]
    fn genpass_input_numbers_only() {
        let pwd = process_genpass(10, false, false, true, false).expect("valid opts");
        assert_eq!(pwd.len(), 10);
        assert!(pwd.bytes().all(|b| NUMBER.contains(&b)));
    }
}
