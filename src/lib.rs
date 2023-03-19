mod block_ffm;
mod block_helpers;
mod block_loss_functions;
mod block_lr;
mod block_misc;
mod block_neural;
mod block_normalize;
mod block_relu;
mod cache;
mod cmdline;
mod consts;
mod feature_buffer;
mod feature_transform_executor;
mod feature_transform_implementations;
mod feature_transform_parser;
mod graph;
mod model_instance;
mod multithread_helpers;
mod optimizer;
mod parser;
mod persistence;
mod port_buffer;
mod regressor;
mod serving;
mod version;
mod vwmap;

extern crate blas;
extern crate intel_mkl_src;

use shellwords;
use std::ffi::CStr;
use std::io::Cursor;
use std::os::raw::c_char;
use std::thread;
use std::thread::JoinHandle;
use crossbeam::channel::{Receiver, Sender};
use crate::feature_buffer::FeatureBufferTranslator;
use crate::multithread_helpers::BoxedRegressorTrait;
use crate::parser::VowpalParser;
use crate::port_buffer::PortBuffer;
use crate::model_instance::ModelInstance;
use crate::regressor::Regressor;
use crate::vwmap::VwNamespaceMap;

static CHANNEL_CAPACITY: usize = 100_000;

#[repr(C)]
pub struct FfiConcurrentPredictor {
    _marker: core::marker::PhantomData<ConcurrentPredictor>,
}

pub struct ConcurrentPredictor {
    sender: Sender<String>
}

#[repr(C)]
pub struct FfiPredictor {
    _marker: core::marker::PhantomData<Predictor>,
}

pub struct Predictor {
    feature_buffer_translator: FeatureBufferTranslator,
    vw_parser: VowpalParser,
    regressor: BoxedRegressorTrait,
    pb: PortBuffer,
}

impl Predictor {
    unsafe fn predict(&mut self, input_buffer: &str) -> f32 {
        let mut buffered_input = Cursor::new(input_buffer);
        let reading_result = self.vw_parser.next_vowpal(&mut buffered_input);
        let buffer = match reading_result {
            Ok([]) => return -1.0, // EOF
            Ok(buffer2) => buffer2,
            Err(_e) => return -1.0,
        };
        self.feature_buffer_translator.translate(buffer, 0);
        self.regressor
            .predict(&self.feature_buffer_translator.feature_buffer, &mut self.pb)
    }
}

impl ConcurrentPredictor {
    unsafe fn predict(&mut self, input_buffer: &str) -> f32 {
        self.sender.send(input_buffer.to_owned()).unwrap()
    }
}

#[no_mangle]
pub unsafe extern "C" fn new_fw_multi_predictor(command: *const c_char, num_workers: usize) -> *mut FfiConcurrentPredictor {
    // create a predictor that loads the weights file, creates worker threads and can process
    // multiple input rows concurrently 
    let str_command = c_char_to_str(command);
    let words = shellwords::split(str_command).unwrap();
    let cmd_matches = cmdline::create_expected_args().get_matches_from(words);
    let weights_filename = match cmd_matches.value_of("initial_regressor") {
        Some(filename) => filename,
        None => panic!("Cannot resolve input weights file name"),
    };
    let (model_instance, vw_namespace_map, regressor) =
        persistence::new_regressor_from_filename(weights_filename, true, Some(&cmd_matches))
            .unwrap();
    let (sender, receiver) = channel::bounded(CHANNEL_CAPACITY);
    let prototype = generate_prototype_predictor(&model_instance, &vw_namespace_map, regressor);
    initialize_workers(num_workers, receiver, prototype);
    let concurrent_predictor = ConcurrentPredictor {
        sender
    };
    Box::into_raw(Box::new(concurrent_predictor)).cast()
}

unsafe fn initialize_workers(num_workers: usize, receiver: Receiver<String>, prototype: Predictor) {
    for _ in 0..num_workers {
        let receiver_clone = receiver.clone();
        let lite_predictor = Predictor {
            feature_buffer_translator: prototype.feature_buffer_translator.clone(),
            vw_parser: prototype.vw_parser.clone(),
            regressor: prototype.regressor.clone(),
            pb: prototype.pb.clone(),
        };
        thread::spawn(move || {
            loop {
                match receiver_clone {
                    Ok(input_data) => lite_predictor.predict(&input_data),
                    Err(RecvError) => break // channel was closed
                }
            }
        });
    }
}

#[no_mangle]
pub extern "C" fn new_fw_predictor_prototype(command: *const c_char) -> *mut FfiPredictor {
    // create a "prototype" predictor that loads the weights file. This predictor is expensive, and is intended
    // to only be created once. If additional predictors are needed (e.g. for concurrent work), please
    // use this "prototype" with the clone_lite function, which will create cheap copies
    let str_command = c_char_to_str(command);
    let words = shellwords::split(str_command).unwrap();
    let cmd_matches = cmdline::create_expected_args().get_matches_from(words);
    let weights_filename = match cmd_matches.value_of("initial_regressor") {
        Some(filename) => filename,
        None => panic!("Cannot resolve input weights file name"),
    };
    let (model_instance, vw_namespace_map, regressor) =
        persistence::new_regressor_from_filename(weights_filename, true, Some(&cmd_matches))
            .unwrap();
    let predictor = generate_prototype_predictor(&model_instance, &vw_namespace_map, regressor);
    Box::into_raw(Box::new(predictor)).cast()
}

fn generate_prototype_predictor(model_instance: &ModelInstance, vw_namespace_map: &VwNamespaceMap, regressor: Regressor) -> Predictor {
    let feature_buffer_translator = FeatureBufferTranslator::new(&model_instance);
    let vw_parser = VowpalParser::new(&vw_namespace_map);
    let sharable_regressor = BoxedRegressorTrait::new(Box::new(regressor));
    let pb = sharable_regressor.new_portbuffer();
    let predictor = Predictor {
        feature_buffer_translator,
        vw_parser,
        regressor: sharable_regressor,
        pb,
    };
    predictor
}

#[no_mangle]
pub unsafe extern "C" fn clone_lite(prototype: *mut FfiPredictor) -> *mut FfiPredictor {
    // given an expensive "prototype" predictor, this function creates cheap copies of it
    // that can be used in different threads concurrently. Note that individually, these predictors
    // are not thread safe, but it is safe to use multiple threads, each accessing only one predictor.
    let prototype: &mut Predictor = from_ptr(prototype);
    let lite_predictor = Predictor {
        feature_buffer_translator: prototype.feature_buffer_translator.clone(),
        vw_parser: prototype.vw_parser.clone(),
        regressor: prototype.regressor.clone(),
        pb: prototype.pb.clone(),
    };
    Box::into_raw(Box::new(lite_predictor)).cast()
}

#[no_mangle]
pub unsafe extern "C" fn fw_predict(ptr: *mut FfiPredictor, input_buffer: *const c_char) -> f32 {
    let str_buffer = c_char_to_str(input_buffer);
    let predictor: &mut Predictor = from_ptr(ptr);
    predictor.predict(str_buffer)
}

#[no_mangle]
pub unsafe extern "C" fn fw_concurrent_predict(ptr: *mut FfiConcurrentPredictor, input_buffer: *const c_char) -> f32 {
    let str_buffer = c_char_to_str(input_buffer);
    let predictor: &mut ConcurrentPredictor = from_ptr(ptr);
    predictor.predict(str_buffer)
}

#[no_mangle]
pub unsafe extern "C" fn free_predictor(ptr: *mut FfiPredictor) {
    drop::<Box<Predictor>>(Box::from_raw(from_ptr(ptr)));
}

#[no_mangle]
pub unsafe extern "C" fn free_concurrent_predictor(ptr: *mut FfiConcurrentPredictor) {
    drop::<Box<ConcurrentPredictor>>(Box::from_raw(from_ptr(ptr)));
}

unsafe fn from_ptr<'a, FROM, TO>(ptr: *mut FROM) -> &'a mut TO {
    if ptr.is_null() {
        eprintln!("Fatal error, got NULL `Context` pointer");
        std::process::abort();
    }
    &mut *(ptr.cast())
}

fn c_char_to_str<'a>(input_buffer: *const c_char) -> &'a str {
    let c_str = unsafe {
        assert!(!input_buffer.is_null());
        CStr::from_ptr(input_buffer)
    };
    let str_buffer = c_str.to_str().unwrap();
    str_buffer
}
