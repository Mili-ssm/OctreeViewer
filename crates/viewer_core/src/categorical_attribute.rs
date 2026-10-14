//! Explicit, bit-exact categorization of complete numeric tuples before building masks.
use crate::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, DatasetError,
};
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CategoricalAttributeError {
    #[error("categorical domain exceeds u32")]
    TooManyCategories,
    #[error(transparent)]
    Dataset(#[from] DatasetError),
}

/// Dense source-row codes and code -> original typed tuple, in first-occurrence order.
/// Scope and equality are explicit: per supplied column, complete little-endian byte tuples.
/// Includes distinct NaN payloads and signed zeros if the caller chooses to categorize floats.
/// No semantic role is inferred and no source column is modified.
#[derive(Debug, Clone, PartialEq)]
pub struct CategoricalAttribute {
    codes: AttributeColumn,
    dictionary: AttributeColumn,
    category_count: u32,
}
impl CategoricalAttribute {
    pub fn new(source: &AttributeColumn) -> Result<Self, CategoricalAttributeError> {
        let components = source.descriptor().component_count() as usize;
        let rows = source.values().len() / components;
        let mut lookup = HashMap::<Vec<u8>, u32>::new();
        let mut unique_rows = Vec::new();
        let mut codes = Vec::with_capacity(rows);
        let mut key = Vec::new();
        for row in 0..rows {
            key.clear();
            macro_rules! bytes {
                ($v:expr) => {
                    for value in &$v[row * components..(row + 1) * components] {
                        key.extend_from_slice(&value.to_le_bytes());
                    }
                };
            }
            match source.values() {
                AttributeValues::U8(v) => bytes!(v),
                AttributeValues::I8(v) => bytes!(v),
                AttributeValues::U16(v) => bytes!(v),
                AttributeValues::I16(v) => bytes!(v),
                AttributeValues::U32(v) => bytes!(v),
                AttributeValues::I32(v) => bytes!(v),
                AttributeValues::U64(v) => bytes!(v),
                AttributeValues::I64(v) => bytes!(v),
                AttributeValues::F32(v) => bytes!(v),
                AttributeValues::F64(v) => bytes!(v),
            }
            let code = if let Some(&code) = lookup.get(&key) {
                code
            } else {
                let code = u32::try_from(unique_rows.len())
                    .map_err(|_| CategoricalAttributeError::TooManyCategories)?;
                unique_rows.push(row);
                lookup.insert(key.clone(), code);
                code
            };
            codes.push(code);
        }
        let category_count = u32::try_from(unique_rows.len())
            .map_err(|_| CategoricalAttributeError::TooManyCategories)?;
        let dictionary = AttributeColumn::new(
            source.descriptor().clone(),
            source.values().select_rows(&unique_rows, components)?,
            unique_rows.len(),
        )?;
        let descriptor =
            AttributeDescriptor::new(source.descriptor().name(), AttributeDataType::U32, 1)
                .expect("valid source name, nonzero components");
        Ok(Self {
            codes: AttributeColumn::new(descriptor, AttributeValues::U32(codes), rows)?,
            dictionary,
            category_count,
        })
    }
    pub fn codes(&self) -> &AttributeColumn {
        &self.codes
    }
    pub fn dictionary(&self) -> &AttributeColumn {
        &self.dictionary
    }
    pub fn category_count(&self) -> u32 {
        self.category_count
    }
}
