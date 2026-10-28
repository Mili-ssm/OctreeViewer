use crate::FormatError;

pub(crate) struct SliceCursor<'a> {
    pub(crate) bytes: &'a [u8],
    pub(crate) position: usize,
}

impl<'a> SliceCursor<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    pub(crate) fn take(&mut self, length: usize) -> Result<&'a [u8], FormatError> {
        let end = self
            .position
            .checked_add(length)
            .ok_or(FormatError::TruncatedIndex)?;
        let bytes = self
            .bytes
            .get(self.position..end)
            .ok_or(FormatError::TruncatedIndex)?;
        self.position = end;
        Ok(bytes)
    }

    pub(crate) fn remaining(&self) -> usize {
        self.bytes.len() - self.position
    }

    pub(crate) fn remaining_slice(&self) -> &'a [u8] {
        &self.bytes[self.position..]
    }

    pub(crate) fn u8(&mut self) -> Result<u8, FormatError> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn u16(&mut self) -> Result<u16, FormatError> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().expect("fixed width"),
        ))
    }

    pub(crate) fn i8(&mut self) -> Result<i8, FormatError> {
        Ok(self.u8()? as i8)
    }

    pub(crate) fn i16(&mut self) -> Result<i16, FormatError> {
        Ok(i16::from_le_bytes(
            self.take(2)?.try_into().expect("fixed width"),
        ))
    }

    pub(crate) fn u32(&mut self) -> Result<u32, FormatError> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("fixed width"),
        ))
    }

    pub(crate) fn i32(&mut self) -> Result<i32, FormatError> {
        Ok(i32::from_le_bytes(
            self.take(4)?.try_into().expect("fixed width"),
        ))
    }

    pub(crate) fn u64(&mut self) -> Result<u64, FormatError> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().expect("fixed width"),
        ))
    }

    pub(crate) fn i64(&mut self) -> Result<i64, FormatError> {
        Ok(i64::from_le_bytes(
            self.take(8)?.try_into().expect("fixed width"),
        ))
    }

    pub(crate) fn f32(&mut self) -> Result<f32, FormatError> {
        Ok(f32::from_bits(self.u32()?))
    }

    pub(crate) fn f64(&mut self) -> Result<f64, FormatError> {
        Ok(f64::from_bits(self.u64()?))
    }

    pub(crate) fn string(&mut self, max_bytes: u32) -> Result<Option<String>, FormatError> {
        let length = self.u32()?;
        if length == 0 {
            return Ok(None);
        }
        if length > max_bytes {
            return Err(FormatError::StringExceedsLimit {
                requested: length,
                limit: max_bytes,
            });
        }
        let length = length as usize;
        let bytes = self.take(length)?;
        Ok(Some(
            std::str::from_utf8(bytes)
                .map_err(|_| FormatError::InvalidUtf8)?
                .to_owned(),
        ))
    }
}
