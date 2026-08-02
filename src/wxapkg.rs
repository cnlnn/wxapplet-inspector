use aes::Aes256;
use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use pbkdf2::pbkdf2_hmac;
use sha1::Sha1;
use std::{fs, ops::Range, path::Path};

const ENCRYPTED_MAGIC: &[u8] = b"V1MMWX";
const ENCRYPTED_PREFIX_LEN: usize = 1024;
const PBKDF2_SALT: &[u8] = b"saltiest";
const AES_IV: &[u8; 16] = b"the iv: 16 bytes";
const PBKDF2_ITERATIONS: u32 = 1000;

type Aes256CbcDecryptor = cbc::Decryptor<Aes256>;

fn is_appid(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 18 && bytes.starts_with(b"wx") && bytes[2..].iter().all(u8::is_ascii_hexdigit)
}

fn appid_from_path(path: &Path) -> Option<&str> {
    path.ancestors()
        .skip(1)
        .filter_map(Path::file_name)
        .filter_map(|name| name.to_str())
        .find(|value| is_appid(value))
}

fn decrypt(bytes: &[u8], appid: &str) -> Result<Vec<u8>, String> {
    let encrypted_start = ENCRYPTED_MAGIC.len();
    let encrypted_end = encrypted_start + ENCRYPTED_PREFIX_LEN;
    if !bytes.starts_with(ENCRYPTED_MAGIC) || bytes.len() < encrypted_end {
        return Err("加密 wxapkg 数据截断".into());
    }
    let mut key = [0u8; 32];
    pbkdf2_hmac::<Sha1>(appid.as_bytes(), PBKDF2_SALT, PBKDF2_ITERATIONS, &mut key);
    let prefix = Aes256CbcDecryptor::new(&key.into(), AES_IV.into())
        .decrypt_padded_vec_mut::<Pkcs7>(&bytes[encrypted_start..encrypted_end])
        .map_err(|_| "加密 wxapkg 的 AppID 或 AES 数据无效".to_owned())?;
    let xor_key = appid
        .as_bytes()
        .get(appid.len().saturating_sub(2))
        .copied()
        .unwrap_or(b'f');
    let mut decrypted = Vec::with_capacity(prefix.len() + bytes.len() - encrypted_end);
    decrypted.extend(prefix);
    decrypted.extend(bytes[encrypted_end..].iter().map(|byte| byte ^ xor_key));
    Ok(decrypted)
}

#[derive(Debug, Clone)]
struct Entry {
    name: String,
    data: Range<usize>,
}

/// An indexed wxapkg v1 archive. Windows encrypted packages are decrypted in
/// memory before indexing; file bodies stay in one buffer and are borrowed.
pub struct Archive {
    bytes: Vec<u8>,
    entries: Vec<Entry>,
}

impl Archive {
    pub fn open(path: &Path) -> Result<Self, String> {
        let bytes =
            fs::read(path).map_err(|error| format!("读取 {} 失败：{error}", path.display()))?;
        let bytes = if bytes.starts_with(ENCRYPTED_MAGIC) {
            let appid = appid_from_path(path).ok_or("无法从缓存路径确定加密包 AppID")?;
            decrypt(&bytes, appid)?
        } else {
            bytes
        };
        Self::parse(bytes)
    }

    pub fn parse(bytes: Vec<u8>) -> Result<Self, String> {
        if bytes.len() < 18 || bytes[0] != 0xbe || bytes[13] != 0xed {
            return Err("不是受支持的 wxapkg v1 包".into());
        }
        let read_u32 = |offset: usize| -> Result<usize, String> {
            bytes
                .get(offset..offset.saturating_add(4))
                .and_then(|part| part.try_into().ok())
                .map(u32::from_be_bytes)
                .map(|value| value as usize)
                .ok_or_else(|| "wxapkg 索引截断".into())
        };
        let count = read_u32(14)?;
        if count > bytes.len().saturating_sub(18) / 12 {
            return Err("wxapkg 条目数无效".into());
        }

        let mut cursor = 18usize;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let name_length = read_u32(cursor)?;
            cursor = cursor.checked_add(4).ok_or("wxapkg 索引溢出")?;
            let name_end = cursor.checked_add(name_length).ok_or("wxapkg 索引溢出")?;
            let metadata_end = name_end.checked_add(8).ok_or("wxapkg 索引溢出")?;
            if metadata_end > bytes.len() {
                return Err("wxapkg 索引无效".into());
            }
            let name = std::str::from_utf8(&bytes[cursor..name_end])
                .map_err(|_| "wxapkg 文件名不是 UTF-8")?
                .to_owned();
            let offset = read_u32(name_end)?;
            let length = read_u32(name_end + 4)?;
            let end = offset.checked_add(length).ok_or("wxapkg 文件范围溢出")?;
            if end > bytes.len() {
                return Err("wxapkg 文件范围无效".into());
            }
            entries.push(Entry {
                name,
                data: offset..end,
            });
            cursor = metadata_end;
        }
        Ok(Self { bytes, entries })
    }

    pub fn files(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.entries
            .iter()
            .map(|entry| (entry.name.as_str(), &self.bytes[entry.data.clone()]))
    }

    pub fn named(&self, name: &str) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|entry| entry.name.rsplit('/').next() == Some(name))
            .map(|entry| &self.bytes[entry.data.clone()])
    }
}

#[cfg(test)]
pub fn fixture(files: &[(&str, &[u8])]) -> Vec<u8> {
    let table_length: usize = files.iter().map(|(name, _)| 4 + name.len() + 8).sum();
    let mut out = vec![0u8; 18 + table_length];
    out[0] = 0xbe;
    out[13] = 0xed;
    out[14..18].copy_from_slice(&(files.len() as u32).to_be_bytes());
    let mut cursor = 18;
    let mut data_offset = out.len();
    for (name, data) in files {
        out[cursor..cursor + 4].copy_from_slice(&(name.len() as u32).to_be_bytes());
        cursor += 4;
        out[cursor..cursor + name.len()].copy_from_slice(name.as_bytes());
        cursor += name.len();
        out[cursor..cursor + 4].copy_from_slice(&(data_offset as u32).to_be_bytes());
        out[cursor + 4..cursor + 8].copy_from_slice(&(data.len() as u32).to_be_bytes());
        cursor += 8;
        data_offset += data.len();
    }
    for (_, data) in files {
        out.extend_from_slice(data);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use cbc::cipher::BlockEncryptMut;

    fn encrypt_fixture(plain: &[u8], appid: &str) -> Vec<u8> {
        assert!(plain.len() > 1023);
        let mut key = [0u8; 32];
        pbkdf2_hmac::<Sha1>(appid.as_bytes(), PBKDF2_SALT, PBKDF2_ITERATIONS, &mut key);
        let prefix = cbc::Encryptor::<Aes256>::new(&key.into(), AES_IV.into())
            .encrypt_padded_vec_mut::<Pkcs7>(&plain[..1023]);
        assert_eq!(prefix.len(), ENCRYPTED_PREFIX_LEN);
        let xor_key = appid.as_bytes()[appid.len() - 2];
        let mut encrypted = ENCRYPTED_MAGIC.to_vec();
        encrypted.extend(prefix);
        encrypted.extend(plain[1023..].iter().map(|byte| byte ^ xor_key));
        encrypted
    }

    #[test]
    fn opens_encrypted_windows_package_using_appid_from_path() {
        let root = std::env::temp_dir().join(format!(
            "wxapkg-encrypted-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let appid = "wx0123456789abcdef";
        let package = root.join(format!("packages/{appid}/1/__APP__.wxapkg"));
        fs::create_dir_all(package.parent().unwrap()).unwrap();
        let body = vec![b'x'; 1600];
        let plain = fixture(&[("app-config.json", &body)]);
        fs::write(&package, encrypt_fixture(&plain, appid)).unwrap();

        let archive = Archive::open(&package).unwrap();
        assert_eq!(archive.named("app-config.json"), Some(body.as_slice()));
        let _ = fs::remove_dir_all(root);
    }
}
