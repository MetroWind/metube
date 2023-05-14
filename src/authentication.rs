use time::OffsetDateTime;
use sha2::Digest;
use base64::engine::Engine;

use crate::error::Error;

static BASE64: &base64::engine::general_purpose::GeneralPurpose =
    &base64::engine::general_purpose::STANDARD_NO_PAD;

#[derive(PartialEq)]
#[cfg_attr(test, derive(Debug))]
enum NonceCheck
{
    Pass, Stale, Fail
}

pub struct DigestAuthentication
{
    secret: String,
    auth_timeout: time::Duration,
}

impl DigestAuthentication
{
    pub fn new(secret: String, auth_timeout: time::Duration) -> Self
    {
        Self { secret, auth_timeout }
    }

    pub fn newNonce(&self) -> String
    {
        self.calculateNonce(OffsetDateTime::now_utc().unix_timestamp_nanos())
    }

    fn hashTimestamp(&self, ts_str: &str) -> String
    {
        let to_hash = format!("{}:{}", ts_str, self.secret);
        let mut hasher = sha2::Sha256::new();
        hasher.update(to_hash.as_bytes());
        let hash_byte_strs: Vec<_> = hasher.finalize().iter()
            .map(|b| format!("{:02x}", b)).collect();
        hash_byte_strs.join("")
    }

    fn calculateNonce(&self, timestamp_nano: i128) -> String
    {
        let ts_str = format!("{:016x}", timestamp_nano);
        let hash_str = self.hashTimestamp(&ts_str);
        BASE64.encode(&format!("{} {}", ts_str, hash_str).as_bytes())
    }

    pub fn checkNonce(&self, nonce: &str) -> NonceCheck
    {
        let nonce_decoded = if let Ok(b) = BASE64.decode(&nonce)
        {
            b
        }
        else
        {
            return NonceCheck::Fail;
        };
        let nonce_decoded = if let Ok(s) = String::from_utf8(nonce_decoded)
        {
            s
        }
        else
        {
            return NonceCheck::Fail;
        };
        let mut split = nonce_decoded.splitn(2, " ");
        let ts_str = if let Some(s) = split.next()
        {
            s
        }
        else
        {
            return NonceCheck::Fail;
        };
        if ts_str.bytes().len() != 16
        {
            return NonceCheck::Fail;
        }
        let hash = if let Some(s) = split.next()
        {
            s
        }
        else
        {
            return NonceCheck::Fail;
        };
        if self.hashTimestamp(ts_str) == hash
        {
            let ts = if let Ok(x) = i128::from_str_radix(ts_str, 16)
            {
                x
            }
            else
            {
                return NonceCheck::Fail;
            };
            let auth_time = if let Ok(t) =
                OffsetDateTime::from_unix_timestamp_nanos(ts)
            {
                t
            }
            else
            {
                return NonceCheck::Fail;
            };
            let time_delta = OffsetDateTime::now_utc() - auth_time;
            if time_delta.is_negative()
            {
                return NonceCheck::Fail;
            }
            if time_delta <= self.auth_timeout
            {
                NonceCheck::Pass
            }
            else
            {
                NonceCheck::Stale
            }
        }
        else
        {
            NonceCheck::Fail
        }
    }
}

#[cfg(test)]
mod tests
{
    use super::*;

    #[test]
    fn generateNonceAndCheck()
    {
        let auth = DigestAuthentication::new(
            "123".to_owned(), time::Duration::minutes(1));
        let nonce = auth.newNonce();
        assert_eq!(auth.checkNonce(&nonce), NonceCheck::Pass);
        assert_eq!(auth.checkNonce(""), NonceCheck::Fail);
        assert_eq!(auth.checkNonce("abc"), NonceCheck::Fail);

        let auth = DigestAuthentication::new(
            "123".to_owned(), time::Duration::new(0, 0));
        let nonce = auth.newNonce();
        assert_eq!(auth.checkNonce(&nonce), NonceCheck::Stale);

        let auth1 = DigestAuthentication::new(
            "123".to_owned(), time::Duration::minutes(1));
        let nonce = auth1.newNonce();
        let auth2 = DigestAuthentication::new(
            "124".to_owned(), time::Duration::minutes(1));
        assert_eq!(auth2.checkNonce(&nonce), NonceCheck::Fail);
    }
}
