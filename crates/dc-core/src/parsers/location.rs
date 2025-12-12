/// Converter from byte offsets to line and column numbers
/// Used for accurate conversion of TextSize from rustpython-parser to Location
pub struct LocationConverter {
    source: String,
    line_starts: Vec<usize>,
}

impl LocationConverter {
    /// Creates a new LocationConverter from source code
    pub fn new(source: String) -> Self {
        let line_starts = Self::calculate_line_starts(&source);
        Self {
            source,
            line_starts,
        }
    }

    /// Converts byte offset to line and column number (1-based)
    pub fn byte_offset_to_location(&self, offset: usize) -> (usize, usize) {
        if offset > self.source.len() {
            // If offset is out of bounds, return the last line
            let last_line = self.line_starts.len().max(1);
            let last_col = self
                .source
                .len()
                .saturating_sub(*self.line_starts.last().unwrap_or(&0))
                .max(1);
            return (last_line, last_col);
        }

        // Binary search for the line containing offset
        let (line, line_start_pos) = match self.line_starts.binary_search(&offset) {
            Ok(idx) => {
                // Exact match - start of line idx+1 (1-based)
                (idx + 1, self.line_starts[idx])
            }
            Err(idx) => {
                // idx indicates insertion position
                // offset is in line idx (1-based)
                let line_num = idx.max(1);
                let line_start = if idx == 0 {
                    0
                } else {
                    self.line_starts[idx - 1]
                };
                (line_num, line_start)
            }
        };

        let column = offset.saturating_sub(line_start_pos) + 1;
        (line, column)
    }

    /// Calculates the start positions of each line (in bytes)
    fn calculate_line_starts(source: &str) -> Vec<usize> {
        let mut line_starts = vec![0]; // First line starts at 0
        let mut current_pos = 0;

        for byte in source.bytes() {
            current_pos += 1;
            if byte == b'\n' {
                line_starts.push(current_pos);
            }
        }

        line_starts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_conversion() {
        let source = "line1\nline2\nline3".to_string();
        let converter = LocationConverter::new(source);

        // Start of first line
        assert_eq!(converter.byte_offset_to_location(0), (1, 1));

        // End of first line (before \n)
        assert_eq!(converter.byte_offset_to_location(5), (1, 6));

        // Start of second line (after \n)
        assert_eq!(converter.byte_offset_to_location(6), (2, 1));

        // Middle of second line
        assert_eq!(converter.byte_offset_to_location(8), (2, 3));
    }

    #[test]
    fn test_empty_source() {
        let source = String::new();
        let converter = LocationConverter::new(source);
        assert_eq!(converter.byte_offset_to_location(0), (1, 1));
    }

    #[test]
    fn test_offset_out_of_bounds() {
        let source = "line1\nline2".to_string();
        let converter = LocationConverter::new(source);

        // offset out of bounds
        let (line, col) = converter.byte_offset_to_location(1000);
        assert!(line >= 1);
        assert!(col >= 1);
    }
}
