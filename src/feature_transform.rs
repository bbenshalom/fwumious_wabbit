use crate::model_instance;
use crate::parser;
use crate::vwmap;
use std::error::Error;
use std::io::Error as IOError;
use std::io::ErrorKind;

use fasthash::murmur3;
use serde::{Serialize,Deserialize};

use regex::Regex;
use lazy_static::lazy_static; 

lazy_static! {
    static ref NAMESPACE_TRANSFORM_REGEX: Regex = Regex::new(r"^(.)=(\w+)\((.)\)$").unwrap();
}




// this is macro, globally exported
use crate::feature_reader;
use crate::feature_reader_float_namespace;

pub const TRANSFORM_NAMESPACE_MARK: u32 = 1<< 31;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct NamespaceTransform {
    pub to_namespace_char: char,
    pub to_namespace_index: u32,
    pub from_namespace_char: char,
    pub from_namespace_index: u32, 
    pub function: TransformFunction,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Copy)]
pub enum TransformFunction {
    Sqrt = 1,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct TransformNamespaces {
    pub v: Vec<NamespaceTransform>
}

impl TransformNamespaces {
    pub fn new() -> TransformNamespaces {
        TransformNamespaces {v: Vec::new()}
    }


    pub fn add_transform_namespace(&mut self, vw: &vwmap::VwNamespaceMap, s: &str) -> Result<(), Box<dyn Error>> {
        // This is super super simple parser... more complicated stuff TBD
        
        for cap in NAMESPACE_TRANSFORM_REGEX.captures_iter(s) {
            let from_namespace_char = cap[3].chars().nth(0).unwrap();
            let from_namespace_index = get_namespace_id(self, vw, from_namespace_char)?;
            println!("to: {}, func: {} from: {}({})", &cap[1], &cap[2], from_namespace_char, from_namespace_index);
            
            if !vw.lookup_char_to_save_as_float[from_namespace_char as usize] {
                return Err(Box::new(IOError::new(ErrorKind::Other, format!("Issue in parsing {}: From namespace ({}) has to be defined as --float_namespaces", s, from_namespace_char))));
            }

            if from_namespace_index & TRANSFORM_NAMESPACE_MARK != 0 {
                return Err(Box::new(IOError::new(ErrorKind::Other, format!("Issue in parsing {}: From namespace ({}) cannot be an already transformed namespace", s, from_namespace_char))));
            }

            
            let to_namespace_char = cap[1].chars().nth(0).unwrap();
            let to_namespace_index = get_namespace_id(self, vw, to_namespace_char);
            if to_namespace_index.is_ok() {
                return Err(Box::new(IOError::new(ErrorKind::Other, format!("To namespace of {} already exists: {:?}", s, to_namespace_char))));
            }
            let to_namespace_index = self.v.len() as u32 | TRANSFORM_NAMESPACE_MARK; // mark it as special
            
            let function_str = &cap[2];

            let function = match function_str {
                "sqrt" => TransformFunction::Sqrt,
                _ => return Err(Box::new(IOError::new(ErrorKind::Other, format!("to namespace of {} has unknown transform function {}", s, function_str)))),
            };

            // Now we need to add it
            let nt = NamespaceTransform {
                from_namespace_char: from_namespace_char,
                from_namespace_index: from_namespace_index,
                to_namespace_char: to_namespace_char,
                to_namespace_index: to_namespace_index,
                function: function    
            };
        
            self.v.push(nt);

        }
        Ok(())
    }
}

pub fn get_namespace_id(transform_namespaces: &TransformNamespaces, vw: &vwmap::VwNamespaceMap, namespace_char: char) -> Result<u32, Box<dyn Error>> {
   let index = match vw.map_char_to_index.get(&namespace_char) {
       Some(index) => return Ok(*index as u32),
       None => {
           let f:Vec<&NamespaceTransform> = transform_namespaces.v.iter().filter(|x| x.to_namespace_char == namespace_char).collect();
           if f.len() == 0 {
               return Err(Box::new(IOError::new(ErrorKind::Other, format!("Unknown namespace char in command line: {}", namespace_char))));
           } else {
               return Ok(f[0].to_namespace_index as u32);
           }
       }
   };   
}




#[inline(always)]
fn emit_u32(hash_data:u32, hash_value:f32, namespace_seed: u32) -> (u32, f32) {
    (murmur3::hash32_with_seed(hash_data.to_le_bytes(), namespace_seed) & parser::MASK31, hash_value)
}                                                         


pub fn transformed_feature<'a>(record_buffer: &[u32], mi: &model_instance::ModelInstance, feature_index_offset: u32) -> Vec<(u32, f32)> {
    // This is FAAAR from optimized
    let mut output:Vec<(u32, f32)> = Vec::new();
    let feature_index_offset = feature_index_offset & !TRANSFORM_NAMESPACE_MARK; // remove transform namespace mark
    //println!("Fi {}", feature_index_offset);
    let transform_namespace = &mi.transform_namespaces.v[feature_index_offset as usize];
    if transform_namespace.function == TransformFunction::Sqrt {
        feature_reader_float_namespace!(record_buffer, transform_namespace.from_namespace_index, hash_data, hash_value, float_value, {
            let transformed_float = float_value.sqrt();
            let transformed_int = transformed_float as u32;
            output.push(emit_u32(transformed_int, hash_value, 0));
            //println!("Input hash value {}, float value {}", hash_value, float_value);
            //println!("Sqrt: {}, sqrt_int {}", transformed_float, transformed_int);
            //println!("Output hash data {}, hash_value {}", output.last().unwrap().0, output.last().unwrap().1);
              
        });
    }

    output
}














