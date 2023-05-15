use sha2::Digest;

pub fn sha256Hash(bytes: &[u8]) -> String
{
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    let hash_byte_strs: Vec<_> = hasher.finalize().iter()
        .map(|b| format!("{:02x}", b)).collect();
    hash_byte_strs.join("")
}
