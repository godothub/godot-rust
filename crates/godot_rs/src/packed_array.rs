extern crate alloc;

use alloc::{string::String, vec::Vec};
use core::ops::{Deref, DerefMut};

use crate::math::{Color, Vector2, Vector3, Vector4};

const STRING_HEADER_BYTES: usize = core::mem::size_of::<u64>();

macro_rules! packed_numeric_array {
    ($name:ident, $element:ty) => {
        #[doc = concat!("Owned Rust representation of Godot `", stringify!($name), "`.")]
        #[derive(Clone, Debug, Default, PartialEq)]
        pub struct $name(Vec<$element>);

        impl $name {
            #[must_use]
            pub const fn new() -> Self {
                Self(Vec::new())
            }

            #[must_use]
            pub fn from_vec(values: Vec<$element>) -> Self {
                Self(values)
            }

            #[must_use]
            pub fn into_vec(self) -> Vec<$element> {
                self.0
            }

            #[doc(hidden)]
            #[must_use]
            pub fn __bytes(&self) -> &[u8] {
                // SAFETY: Every supported element is a repr(C) aggregate of
                // integers or floats with an authenticated padding-free Godot
                // layout. Reading initialized values as bytes is always valid.
                unsafe {
                    core::slice::from_raw_parts(
                        self.0.as_ptr().cast::<u8>(),
                        core::mem::size_of_val(self.0.as_slice()),
                    )
                }
            }

            #[doc(hidden)]
            pub fn __from_bytes(bytes: &[u8]) -> Option<Self> {
                let width = core::mem::size_of::<$element>();
                if bytes.len() % width != 0 {
                    return None;
                }
                let values = bytes
                    .chunks_exact(width)
                    .map(|chunk| {
                        // SAFETY: The exact element width was checked. All bit
                        // patterns are valid for these integer/float aggregates,
                        // and unaligned input is copied into owned Rust storage.
                        unsafe { core::ptr::read_unaligned(chunk.as_ptr().cast::<$element>()) }
                    })
                    .collect();
                Some(Self(values))
            }
        }

        impl From<Vec<$element>> for $name {
            fn from(values: Vec<$element>) -> Self {
                Self::from_vec(values)
            }
        }

        impl From<$name> for Vec<$element> {
            fn from(values: $name) -> Self {
                values.into_vec()
            }
        }

        impl AsRef<[$element]> for $name {
            fn as_ref(&self) -> &[$element] {
                &self.0
            }
        }

        impl Deref for $name {
            type Target = [$element];

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl DerefMut for $name {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.0
            }
        }
    };
}

packed_numeric_array!(PackedByteArray, u8);
packed_numeric_array!(PackedInt32Array, i32);
packed_numeric_array!(PackedInt64Array, i64);
packed_numeric_array!(PackedFloat32Array, f32);
packed_numeric_array!(PackedFloat64Array, f64);
packed_numeric_array!(PackedVector2Array, Vector2);
packed_numeric_array!(PackedVector3Array, Vector3);
packed_numeric_array!(PackedVector4Array, Vector4);
packed_numeric_array!(PackedColorArray, Color);

/// Owned UTF-8 representation of Godot `PackedStringArray`.
///
/// The encoded backing lets generated engine calls borrow one stable range
/// without allocating at the Host boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackedStringArray {
    encoded: Vec<u8>,
}

impl PackedStringArray {
    #[must_use]
    pub fn new() -> Self {
        Self {
            encoded: 0_u64.to_le_bytes().to_vec(),
        }
    }

    #[must_use]
    pub fn from_vec(values: Vec<String>) -> Self {
        let mut encoded = Vec::with_capacity(
            STRING_HEADER_BYTES
                + values
                    .iter()
                    .map(|value| STRING_HEADER_BYTES + value.len())
                    .sum::<usize>(),
        );
        encoded.extend_from_slice(&(values.len() as u64).to_le_bytes());
        for value in values {
            encoded.extend_from_slice(&(value.len() as u64).to_le_bytes());
            encoded.extend_from_slice(value.as_bytes());
        }
        Self { encoded }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        read_u64(&self.encoded, 0)
            .and_then(|value| usize::try_from(value).ok())
            .expect("PackedStringArray maintains a valid count")
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[must_use]
    pub fn get(&self, index: usize) -> Option<&str> {
        self.iter().nth(index)
    }

    pub fn push(&mut self, value: impl AsRef<str>) {
        let value = value.as_ref();
        let count = self.len();
        self.encoded[..STRING_HEADER_BYTES].copy_from_slice(&((count + 1) as u64).to_le_bytes());
        self.encoded
            .extend_from_slice(&(value.len() as u64).to_le_bytes());
        self.encoded.extend_from_slice(value.as_bytes());
    }

    pub fn iter(&self) -> PackedStringIter<'_> {
        PackedStringIter {
            bytes: &self.encoded,
            remaining: self.len(),
            offset: STRING_HEADER_BYTES,
        }
    }

    #[must_use]
    pub fn to_vec(&self) -> Vec<String> {
        self.iter().map(str::to_owned).collect()
    }

    #[doc(hidden)]
    #[must_use]
    pub fn __bytes(&self) -> &[u8] {
        &self.encoded
    }

    #[doc(hidden)]
    pub fn __from_bytes(bytes: &[u8]) -> Option<Self> {
        validate_strings(bytes)?;
        Some(Self {
            encoded: bytes.to_vec(),
        })
    }
}

impl Default for PackedStringArray {
    fn default() -> Self {
        Self::new()
    }
}

impl From<Vec<String>> for PackedStringArray {
    fn from(values: Vec<String>) -> Self {
        Self::from_vec(values)
    }
}

impl From<PackedStringArray> for Vec<String> {
    fn from(values: PackedStringArray) -> Self {
        values.to_vec()
    }
}

impl<'a> IntoIterator for &'a PackedStringArray {
    type Item = &'a str;
    type IntoIter = PackedStringIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

pub struct PackedStringIter<'a> {
    bytes: &'a [u8],
    remaining: usize,
    offset: usize,
}

impl<'a> Iterator for PackedStringIter<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let length = usize::try_from(read_u64(self.bytes, self.offset)?).ok()?;
        self.offset = self.offset.checked_add(STRING_HEADER_BYTES)?;
        let end = self.offset.checked_add(length)?;
        let value = core::str::from_utf8(self.bytes.get(self.offset..end)?).ok()?;
        self.offset = end;
        self.remaining -= 1;
        Some(value)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl ExactSizeIterator for PackedStringIter<'_> {}

fn validate_strings(bytes: &[u8]) -> Option<()> {
    let count = usize::try_from(read_u64(bytes, 0)?).ok()?;
    let mut offset = STRING_HEADER_BYTES;
    for _ in 0..count {
        let length = usize::try_from(read_u64(bytes, offset)?).ok()?;
        offset = offset.checked_add(STRING_HEADER_BYTES)?;
        let end = offset.checked_add(length)?;
        core::str::from_utf8(bytes.get(offset..end)?).ok()?;
        offset = end;
    }
    (offset == bytes.len()).then_some(())
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    let end = offset.checked_add(STRING_HEADER_BYTES)?;
    Some(u64::from_le_bytes(bytes.get(offset..end)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_arrays_round_trip_unaligned_bytes() {
        let values = PackedVector3Array::from(vec![
            Vector3::new(1.0, 2.0, 3.0),
            Vector3::new(-4.0, 5.0, -6.0),
        ]);
        let mut unaligned = vec![0_u8];
        unaligned.extend_from_slice(values.__bytes());
        let restored = PackedVector3Array::__from_bytes(&unaligned[1..]).expect("valid bytes");
        assert_eq!(restored, values);
        assert!(PackedVector3Array::__from_bytes(&unaligned[1..unaligned.len() - 1]).is_none());
    }

    #[test]
    fn numeric_elements_match_godot_standard_precision_layouts() {
        assert_eq!(core::mem::size_of::<Vector2>(), 8);
        assert_eq!(core::mem::size_of::<Vector3>(), 12);
        assert_eq!(core::mem::size_of::<Vector4>(), 16);
        assert_eq!(core::mem::size_of::<Color>(), 16);
        assert_eq!(core::mem::align_of::<Vector2>(), 4);
        assert_eq!(core::mem::align_of::<Vector3>(), 4);
        assert_eq!(core::mem::align_of::<Vector4>(), 4);
        assert_eq!(core::mem::align_of::<Color>(), 4);
    }

    #[test]
    fn string_arrays_validate_utf8_and_canonical_lengths() {
        let mut values = PackedStringArray::from(vec![String::from("你好"), String::from("Godot")]);
        values.push("玩家");
        assert_eq!(values.len(), 3);
        assert_eq!(values.get(0), Some("你好"));
        assert_eq!(values.to_vec(), ["你好", "Godot", "玩家"]);
        assert_eq!(
            PackedStringArray::__from_bytes(values.__bytes()),
            Some(values.clone())
        );

        let mut trailing = values.__bytes().to_vec();
        trailing.push(0);
        assert!(PackedStringArray::__from_bytes(&trailing).is_none());
        let mut invalid_utf8 = PackedStringArray::new().__bytes().to_vec();
        invalid_utf8[..8].copy_from_slice(&1_u64.to_le_bytes());
        invalid_utf8.extend_from_slice(&1_u64.to_le_bytes());
        invalid_utf8.push(0xff);
        assert!(PackedStringArray::__from_bytes(&invalid_utf8).is_none());
    }
}
