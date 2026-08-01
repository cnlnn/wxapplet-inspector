use std::{fs, ops::Range, path::Path};

#[derive(Debug, Clone)]
struct Entry {
    name: String,
    data: Range<usize>,
}

/// An indexed, unencrypted wxapkg v1 archive. File bodies stay in one buffer
/// and are borrowed by callers instead of copied for every archive entry.
pub struct Archive {
    bytes: Vec<u8>,
    entries: Vec<Entry>,
}

impl Archive {
    pub fn open(path: &Path) -> Result<Self, String> {
        Self::parse(
            fs::read(path).map_err(|error| format!("读取 {} 失败：{error}", path.display()))?,
        )
    }

    pub fn parse(bytes: Vec<u8>) -> Result<Self, String> {
        if bytes.len() < 18 || bytes[0] != 0xbe || bytes[13] != 0xed {
            return Err("不是未加密 wxapkg v1 包".into());
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
