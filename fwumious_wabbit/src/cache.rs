
use std::{mem,slice};
use std::io;
use std::io::Read;
use std::fs;
use std::error::Error;
use std::path;
use flate2::write::DeflateEncoder;
use flate2::Compression;
use flate2::read::DeflateDecoder;
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};

use crate::parser;
use crate::vwmap;
use crate::persistence;

const CACHE_HEADER_MAGIC_STRING: &[u8; 4] = b"FWCA";    // Fwumious Wabbit CAche
const CACHE_HEADER_VERSION:u32 = 4;
// Cache layout:
// 4 bytes: Magic bytes
// u32: Version of the cache format
// u_size + length - json encoding of vw_source
// ...cached examples


const READBUF_LEN:usize = 1024*100;


pub struct RecordCache {
    output_bufwriter: Box<dyn io::Write>,
    input_bufreader: Box<dyn io::Read>,
    temporary_filename: String,
    final_filename: String,
    pub writing: bool,
    pub reading: bool,
//    pub output_buffer: Vec<u32>,
    pub byte_buffer: Vec<u8>,//[u8; READBUF_LEN],
    start_pointer: usize,
    end_pointer: usize,
    total_read: usize,
}


impl RecordCache {
    pub fn new(input_filename: &str, enabled: bool, vw_map: &vwmap::VwNamespaceMap) -> RecordCache {
        let temporary_filename: String;
        let final_filename: String;
        let gz: bool;
        temporary_filename = format!("{}.fwcache.writing", input_filename);
        final_filename = format!("{}.fwcache", input_filename);
        if !input_filename.ends_with("gz") {
            gz = false;
        } else
        {
            gz = true;
        }
        
        let mut rc = RecordCache {
            output_bufwriter: Box::new(io::BufWriter::new(io::sink())),
            input_bufreader: Box::new(io::empty()),
            temporary_filename: temporary_filename.to_string(),
            final_filename: final_filename.to_string(),
            writing: false,
            reading: false,
            byte_buffer: Vec::new(),
            start_pointer: 0,
            end_pointer: 0,
            total_read: 0,
        };
        
        if enabled {
            if path::Path::new(&final_filename).exists() {
                rc.reading = true;
                if !gz {
                    // we buffer ourselves, otherwise i would be wise to use bufreader
                    rc.input_bufreader = Box::new(fs::File::open(&final_filename).unwrap());
                } else {
                    rc.input_bufreader = Box::new(DeflateDecoder::new(fs::File::open(&final_filename).unwrap()));
                }
                println!("using cache_file = {}", final_filename );
                println!("ignoring text input in favor of cache input");
                rc.byte_buffer.resize(READBUF_LEN, 0);
                match rc.verify_header(vw_map) {
                    Ok(()) => {},
                    Err(e) => {
                        
                        println!("Couldn't use the existing cache file: {:?}", e);
                        rc.reading = false;
                    }
                }
            }
            if !rc.reading {
                rc.writing = true;
                println!("creating cache file = {}", final_filename );
                if !gz {
                    rc.output_bufwriter = Box::new(io::BufWriter::new(fs::File::create(temporary_filename).unwrap()));
                } else {
                    rc.output_bufwriter = Box::new(io::BufWriter::new(DeflateEncoder::new(fs::File::create(temporary_filename).unwrap(),
                                                                    Compression::fast())));
                }
                rc.write_header(vw_map).unwrap();
            }
        }        
        rc
    }
    
    pub fn push_record(&mut self, record_buf: &[u32]) -> Result<(), Box<dyn Error>> {
        if self.writing {
            let element_size = mem::size_of::<u32>();
            let mut vv:&[u8];
            unsafe { 
                vv = slice::from_raw_parts(record_buf.as_ptr() as *const u8, 
                                            record_buf.len() * element_size) ;
                self.output_bufwriter.write(&vv)?;
            }
        }
        Ok(())
    }
    
    pub fn write_finish(&mut self)  -> Result<(), Box<dyn Error>> {
        if self.writing {
            self.output_bufwriter.flush()?;
            fs::rename(&self.temporary_filename, &self.final_filename)?;
        }
        Ok(())
    }

    pub fn write_header(&mut self, vw_map: &vwmap::VwNamespaceMap) -> Result<(), Box<dyn Error>> {
        self.output_bufwriter.write(CACHE_HEADER_MAGIC_STRING)?;
        self.output_bufwriter.write_u32::<LittleEndian>(CACHE_HEADER_VERSION)?;
        vw_map.save_to_buf(&mut self.output_bufwriter)?;
        Ok(())
    }

    pub fn verify_header(&mut self, vwmap: &vwmap::VwNamespaceMap) -> Result<(), Box<dyn Error>> {
        let mut magic_string: [u8; 4] = [0;4];
        self.input_bufreader.read(&mut magic_string)?;
        if &magic_string != CACHE_HEADER_MAGIC_STRING {
            return Err("Cache header does not begin with magic bytes FWFW")?;
        }
        
        let version = self.input_bufreader.read_u32::<LittleEndian>()?;
        if CACHE_HEADER_VERSION != version {
            return Err(format!("Cache file version of this binary: {}, version of the cache file: {}", CACHE_HEADER_VERSION, version))?;
        }
        
        // Compare vwmap in cache and the one we've been given. If they differ, rebuild cache
        let vwmap_from_cache = vwmap::VwNamespaceMap::new_from_buf(&mut self.input_bufreader)?;
        if vwmap_from_cache.vw_source != vwmap.vw_source {
            return Err("vw_namespace_map.csv and the one from cache file differ")?;
        }
        
        Ok(())
    }
    

    pub fn get_next_record(&mut self) -> Result<&[u32], Box<dyn Error>> {
        if !self.reading {
            return Err("next_recrod() called on reading cache, when not opened in reading mode")?;
        }
        unsafe { 
            // We're going to cast another view over the data, so we can read it as u32
            let buf_view:&[u32] = slice::from_raw_parts(self.byte_buffer.as_ptr() as *const u32, READBUF_LEN/4);
            loop {
                // Classical buffer strategy:
                // Return if you have full record in buffer,
                // Otherwise shift the buffer and backfill it
                if self.end_pointer - self.start_pointer >= 4 {
                    let record_len = buf_view[self.start_pointer /4 ] as usize;
                    if self.start_pointer + record_len * 4 <= self.end_pointer {
                        let ret_buf = &buf_view[self.start_pointer/4..self.start_pointer/4 + record_len];
                        self.start_pointer += record_len * 4;
                        return Ok(ret_buf);
                    }
                } 
                self.byte_buffer.copy_within(self.start_pointer..self.end_pointer, 0);
                self.end_pointer -= self.start_pointer;
                self.start_pointer = 0;

                let read_len = match self.input_bufreader.read(&mut self.byte_buffer[self.end_pointer..READBUF_LEN])  {
                    Ok(0) => return Ok(&[]),
                    Ok(n) => n,
                    Err(e) => Err(e)?          
                };

                self.end_pointer += read_len;
                self.total_read += read_len;
            }            
        }
    }
}