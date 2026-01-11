//! Chunk buffering for streaming body processing.

use crate::errors::MaskingError;

/// Buffer for accumulating body chunks.
#[derive(Debug)]
pub struct ChunkBuffer {
    /// Accumulated data.
    data: Vec<u8>,
    /// Maximum buffer size.
    max_size: usize,
}

impl ChunkBuffer {
    /// Create a new chunk buffer with the specified maximum size.
    pub fn new(max_size: usize) -> Self {
        Self {
            data: Vec::new(),
            max_size,
        }
    }

    /// Append a chunk to the buffer.
    ///
    /// Returns an error if the buffer would exceed the maximum size.
    pub fn append(&mut self, chunk: &[u8]) -> Result<(), MaskingError> {
        if self.data.len() + chunk.len() > self.max_size {
            return Err(MaskingError::BufferOverflow {
                max_bytes: self.max_size,
            });
        }
        self.data.extend_from_slice(chunk);
        Ok(())
    }

    /// Take the accumulated data and reset the buffer.
    pub fn take(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.data)
    }

    /// Get the current size of buffered data.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Check if the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Clear the buffer.
    pub fn clear(&mut self) {
        self.data.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_buffer_append() {
        let mut buffer = ChunkBuffer::new(100);
        assert!(buffer.append(b"hello").is_ok());
        assert!(buffer.append(b" world").is_ok());
        assert_eq!(buffer.len(), 11);
    }

    #[test]
    fn test_buffer_overflow() {
        let mut buffer = ChunkBuffer::new(10);
        assert!(buffer.append(b"hello").is_ok());
        let result = buffer.append(b"world!");
        assert!(matches!(result, Err(MaskingError::BufferOverflow { .. })));
    }

    #[test]
    fn test_buffer_take() {
        let mut buffer = ChunkBuffer::new(100);
        buffer.append(b"test data").unwrap();
        let data = buffer.take();
        assert_eq!(data, b"test data");
        assert!(buffer.is_empty());
    }
}
