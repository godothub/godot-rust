//! Small call buffers shared by generated engine-call backends.
extern crate alloc;
use alloc::vec::Vec;

/// Owns a small argument list without allocating on the common path.
pub struct CallBuffer<T> {
    inline: [Option<T>; 8],
    heap: Vec<T>,
    heap_mode: bool,
    len: usize,
}
impl<T> CallBuffer<T> {
    pub fn new(count: usize) -> Self {
        Self {
            inline: core::array::from_fn(|_| None),
            heap: if count > 8 {
                Vec::with_capacity(count)
            } else {
                Vec::new()
            },
            heap_mode: count > 8,
            len: 0,
        }
    }
    pub fn push(&mut self, value: T) {
        if !self.heap_mode && self.len == 8 {
            self.heap
                .extend(self.inline.iter_mut().filter_map(Option::take));
            self.heap_mode = true;
        }
        if self.heap_mode {
            self.heap.push(value);
        } else {
            self.inline[self.len] = Some(value);
        }
        self.len += 1;
    }
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.inline
            .iter()
            .filter_map(Option::as_ref)
            .chain(self.heap.iter())
    }
    pub fn get(&self, index: usize) -> Option<&T> {
        if self.heap_mode {
            self.heap.get(index)
        } else {
            self.inline.get(index).and_then(Option::as_ref)
        }
    }
    pub fn first(&self) -> Option<&T> {
        self.get(0)
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// Contiguous argument pointers, with heap storage only for large calls.
pub struct CallPointers<T: Copy> {
    inline: [T; 8],
    heap: Vec<T>,
    len: usize,
}
impl<T: Copy> CallPointers<T> {
    pub fn new(count: usize, empty: T) -> Self {
        Self {
            inline: [empty; 8],
            heap: if count > 8 {
                alloc::vec![empty; count]
            } else {
                Vec::new()
            },
            len: count,
        }
    }
    pub fn as_slice(&self) -> &[T] {
        if self.len > 8 {
            &self.heap
        } else {
            &self.inline[..self.len]
        }
    }
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        if self.len > 8 {
            &mut self.heap
        } else {
            &mut self.inline[..self.len]
        }
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn as_ptr(&self) -> *const T {
        self.as_slice().as_ptr()
    }
}

impl<T> core::ops::Index<usize> for CallBuffer<T> {
    type Output = T;
    fn index(&self, index: usize) -> &T {
        self.get(index).expect("call argument index is in bounds")
    }
}
