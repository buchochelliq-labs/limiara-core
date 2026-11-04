#![forbid(unsafe_code)]
use thiserror::Error;

const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";
const PUBLIC_ID_LEN: usize = 18;
const PREFIX_LEN: usize = 3;
const PAYLOAD_LEN: usize = 12;
/// The payload's top bit marks an id a device assigned (ADR 0031). Tenant
/// sequences stay below it, so the two ranges never meet.
const CLIENT_BIT: u64 = 1 << 59;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PublicIdError {
    #[error("public id prefixes must be exactly 3 lowercase letters or digits")]
    InvalidPrefix,
    #[error("tenant-local sequence does not fit in the public id payload")]
    SequenceTooLarge,
    #[error("public id must be 18 lowercase Crockford-base32 characters")]
    InvalidFormat,
    #[error("public id checksum mismatch")]
    InvalidChecksum,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PublicId(String);

impl PublicId {
    pub fn new(prefix: &str, tenant_local_sequence: u64) -> Result<Self, PublicIdError> {
        validate_prefix(prefix)?;
        if tenant_local_sequence >= (1u64 << 60) {
            return Err(PublicIdError::SequenceTooLarge);
        }
        let payload = encode_base32_fixed(tenant_local_sequence, PAYLOAD_LEN);
        let stem = format!("{prefix}{payload}");
        let checksum = checksum(&stem);
        Ok(Self(format!("{stem}{checksum}")))
    }

    /// An id assigned outside the server, by a device creating a record
    /// offline: the client range plus 59 random bits.
    pub fn new_client(prefix: &str, random: u64) -> Result<Self, PublicIdError> {
        Self::new(prefix, CLIENT_BIT | (random & (CLIENT_BIT - 1)))
    }

    /// Whether the id is in the range devices assign.
    pub fn is_client_assigned(&self) -> bool {
        alphabet_value(self.0.as_bytes()[PREFIX_LEN]).is_some_and(|value| value >= 16)
    }

    pub fn parse(input: &str) -> Result<Self, PublicIdError> {
        // Checked first, so slicing by byte below never splits a character.
        if !input.is_ascii()
            || input.len() != PUBLIC_ID_LEN
            || !input[..PREFIX_LEN]
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
            || !input[PREFIX_LEN..].bytes().all(is_public_id_byte)
        {
            return Err(PublicIdError::InvalidFormat);
        }
        let (stem, supplied) = input.split_at(PREFIX_LEN + PAYLOAD_LEN);
        if checksum(stem) != supplied {
            return Err(PublicIdError::InvalidChecksum);
        }
        Ok(Self(input.to_string()))
    }

    pub fn prefix(&self) -> &str {
        &self.0[..PREFIX_LEN]
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn validate_prefix(prefix: &str) -> Result<(), PublicIdError> {
    let valid = prefix.len() == PREFIX_LEN
        && prefix
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    valid.then_some(()).ok_or(PublicIdError::InvalidPrefix)
}

fn encode_base32_fixed(mut value: u64, width: usize) -> String {
    let mut out = vec![b'0'; width];
    for slot in out.iter_mut().rev() {
        *slot = ALPHABET[(value & 31) as usize];
        value >>= 5;
    }
    String::from_utf8(out).expect("base32 alphabet is valid utf8")
}

fn checksum(stem: &str) -> String {
    let mut a = 17u32;
    let mut b = 31u32;
    let mut c = 7u32;
    for byte in stem.bytes() {
        let value = alphabet_value(byte).unwrap_or((byte % 32) as u32);
        a = (a + value) % 32;
        b = (b + a + 3) % 32;
        c = (c + b + value + 5) % 32;
    }
    let bytes = [
        ALPHABET[a as usize],
        ALPHABET[b as usize],
        ALPHABET[c as usize],
    ];
    String::from_utf8(bytes.to_vec()).expect("checksum alphabet is valid utf8")
}

fn is_public_id_byte(byte: u8) -> bool {
    ALPHABET.contains(&byte)
}

fn alphabet_value(byte: u8) -> Option<u32> {
    ALPHABET
        .iter()
        .position(|candidate| *candidate == byte)
        .map(|idx| idx as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_ascii_input_is_rejected_without_panicking() {
        assert_eq!(
            PublicId::parse("aaé00000000000000"),
            Err(PublicIdError::InvalidFormat)
        );
        assert_eq!(
            PublicId::parse("éééééééééééééééééé"),
            Err(PublicIdError::InvalidFormat)
        );
    }

    #[test]
    fn client_ids_never_meet_sequence_ids() {
        let client = PublicId::new_client("0a1", u64::MAX).expect("client id");
        assert!(client.is_client_assigned());
        assert_eq!(PublicId::parse(client.as_str()), Ok(client.clone()));
        assert_eq!(client.prefix(), "0a1");
        let low = PublicId::new_client("0a1", 0).expect("client id");
        assert!(low.is_client_assigned());
        let sequence = PublicId::new("0a1", CLIENT_BIT - 1).expect("largest sequence id");
        assert!(!sequence.is_client_assigned());
        assert!(!PublicId::new("0a1", 1).expect("id").is_client_assigned());
    }

    #[test]
    fn builds_18_character_case_safe_prefixed_ids() {
        let id = PublicId::new("con", 42).expect("valid id");
        assert_eq!(id.as_str().len(), 18);
        assert_eq!(id.prefix(), "con");
        assert!(id.as_str().bytes().all(|b| !b.is_ascii_uppercase()));
        assert_eq!(PublicId::parse(id.as_str()), Ok(id));
    }

    #[test]
    fn round_trips_an_organisation_prefix_with_an_ambiguous_payload_letter() {
        let id = PublicId::new("org", 1).expect("valid organisation ID");
        assert_eq!(PublicId::parse(id.as_str()), Ok(id));
    }

    #[test]
    fn rejects_checksum_mismatch() {
        let id = PublicId::new("per", 99).expect("valid id");
        let mut tampered = id.as_str().to_string();
        tampered.replace_range(5..6, "z");
        assert_eq!(
            PublicId::parse(&tampered),
            Err(PublicIdError::InvalidChecksum)
        );
    }

    #[test]
    fn rejects_ambiguous_or_uppercase_prefixes() {
        assert_eq!(PublicId::new("Con", 1), Err(PublicIdError::InvalidPrefix));
        assert_eq!(
            PublicId::new("contact", 1),
            Err(PublicIdError::InvalidPrefix)
        );
    }
}
