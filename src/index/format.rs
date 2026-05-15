use crate::PACKED_DIMS;

pub struct IndexWriter {
    buf: Vec<u8>,
}

impl IndexWriter {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn write_header(&mut self, reference_count: i32) -> Result<(), String> {
        self.buf.extend_from_slice(b"RNSPCST1"); // magic (8)
        self.write_i32(8192)?; // scale (4)
        self.write_i32(PACKED_DIMS as i32)?; // packed_dims (4)
        self.write_i32(reference_count)?; // reference_count (4)
        // partition_count placeholder (4)
        self.write_i32(0)?;
        // node_count placeholder (4)
        self.write_i32(0)?;
        // block_count placeholder (4)
        self.write_i32(0)?;
        Ok(())
    }

    pub fn write_partition_count(&mut self, count: i32) -> Result<(), String> {
        let offset = 8 + 4 + 4 + 4; // after magic + scale + packed_dims + ref_count
        self.buf[offset..offset + 4].copy_from_slice(&count.to_le_bytes());
        Ok(())
    }

    pub fn write_node_count(&mut self, count: i32) -> Result<(), String> {
        let offset = 8 + 4 + 4 + 4 + 4; // after partition_count
        self.buf[offset..offset + 4].copy_from_slice(&count.to_le_bytes());
        Ok(())
    }

    pub fn write_block_count(&mut self, count: i32) -> Result<(), String> {
        let offset = 8 + 4 + 4 + 4 + 4 + 4; // after node_count
        self.buf[offset..offset + 4].copy_from_slice(&count.to_le_bytes());
        Ok(())
    }

    pub fn write_partition_entry(
        &mut self,
        key: u32,
        root: usize,
        len: usize,
        min: [i16; PACKED_DIMS],
        max: [i16; PACKED_DIMS],
    ) -> Result<(), String> {
        self.write_u32(key)?;
        self.write_i32(root as i32)?;
        self.write_i32(0)?; // start (unused for flat)
        self.write_i32(len as i32)?;
        for &v in &min {
            self.write_i16(v)?;
        }
        for &v in &max {
            self.write_i16(v)?;
        }
        Ok(())
    }

    pub fn write_node_entry(
        &mut self,
        left: i32,
        right: i32,
        start: usize,
        len: usize,
        min: [i16; PACKED_DIMS],
        max: [i16; PACKED_DIMS],
    ) -> Result<(), String> {
        self.write_i32(left)?;
        self.write_i32(right)?;
        self.write_i32(start as i32)?;
        self.write_i32(len as i32)?;
        for &v in &min {
            self.write_i16(v)?;
        }
        for &v in &max {
            self.write_i16(v)?;
        }
        Ok(())
    }

    pub fn write_i16(&mut self, v: i16) -> Result<(), String> {
        self.buf.extend_from_slice(&v.to_le_bytes());
        Ok(())
    }

    pub fn write_u8(&mut self, v: u8) -> Result<(), String> {
        self.buf.push(v);
        Ok(())
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    fn write_u32(&mut self, v: u32) -> Result<(), String> {
        self.buf.extend_from_slice(&v.to_le_bytes());
        Ok(())
    }

    fn write_i32(&mut self, v: i32) -> Result<(), String> {
        self.buf.extend_from_slice(&v.to_le_bytes());
        Ok(())
    }
}
