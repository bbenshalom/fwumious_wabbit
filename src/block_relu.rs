use std::any::Any;
use std::error::Error;

use crate::block_helpers;
use crate::feature_buffer;
use crate::graph;
use crate::graph::BlockGraph;
use crate::model_instance;
use crate::port_buffer;
use crate::regressor;
use regressor::BlockTrait;

pub struct BlockRELU {
    pub num_inputs: usize,
    pub input_offset: usize,
    pub output_offset: usize,
}

pub fn new_relu_block(
    bg: &mut graph::BlockGraph,
    mi: &model_instance::ModelInstance,
    input: graph::BlockPtrOutput,
) -> Result<graph::BlockPtrOutput, Box<dyn Error>> {
    let num_inputs = bg.get_num_output_values(vec![&input]);
    assert!(num_inputs != 0);
    let mut block = Box::new(BlockRELU {
        output_offset: usize::MAX,
        input_offset: usize::MAX,
        num_inputs: num_inputs,
    });
    let mut block_outputs = bg.add_node(block, vec![input])?;
    assert_eq!(block_outputs.len(), 1);
    Ok(block_outputs.pop().unwrap())
}

impl BlockTrait for BlockRELU {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }

    fn allocate_and_init_weights(&mut self, mi: &model_instance::ModelInstance) {}

    fn get_num_output_slots(&self) -> usize {
        1
    }

    fn get_num_output_values(&self, output: graph::OutputSlot) -> usize {
        assert!(output.get_output_index() == 0);
        return self.num_inputs;
    }

    fn set_input_offset(&mut self, input: graph::InputSlot, offset: usize) {
        assert!(input.get_input_index() == 0);
        self.input_offset = offset;
    }

    fn set_output_offset(&mut self, output: graph::OutputSlot, offset: usize) {
        assert!(output.get_output_index() == 0);
        self.output_offset = offset;
    }

    #[inline(always)]
    fn forward_backward(
        &mut self,
        further_blocks: &mut [Box<dyn BlockTrait>],
        fb: &feature_buffer::FeatureBuffer,
        pb: &mut port_buffer::PortBuffer,
        update: bool,
    ) {
        debug_assert!(self.output_offset != usize::MAX);
        debug_assert!(self.input_offset != usize::MAX);
        debug_assert!(self.num_inputs > 0);

        unsafe {
            for i in 0..self.num_inputs as usize {
                let w = *pb.tape.get_unchecked_mut(self.input_offset + i);
                if w < 0.0 {
                    *pb.tape.get_unchecked_mut(self.output_offset + i) = 0.0;
                    *pb.tape.get_unchecked_mut(self.input_offset + i) = 0.0;
                } else {
                    *pb.tape.get_unchecked_mut(self.output_offset + i) = w;
                    *pb.tape.get_unchecked_mut(self.input_offset + i) = 1.0;
                }
            }

            block_helpers::forward_backward(further_blocks, fb, pb, update);

            if update {
                for i in 0..self.num_inputs as usize {
                    let gradient = *pb.tape.get_unchecked(self.output_offset + i);
                    *pb.tape.get_unchecked_mut(self.input_offset + i) *= gradient;
                }
            }
        } // unsafe end
    }

    fn forward(
        &self,
        further_blocks: &[Box<dyn BlockTrait>],
        fb: &feature_buffer::FeatureBuffer,
        pb: &mut port_buffer::PortBuffer,
    ) {
        debug_assert!(self.output_offset != usize::MAX);
        debug_assert!(self.input_offset != usize::MAX);
        debug_assert!(self.num_inputs > 0);

        unsafe {
            for i in 0..self.num_inputs as usize {
                let w = *pb.tape.get_unchecked_mut(self.input_offset + i);
                if w < 0.0 {
                    *pb.tape.get_unchecked_mut(self.output_offset + i) = 0.0;
                } else {
                    *pb.tape.get_unchecked_mut(self.output_offset + i) = w;
                }
            }
            block_helpers::forward(further_blocks, fb, pb);
        } // unsafe end
    }
}

mod tests {
    // Note this useful idiom: importing names from outer (for mod tests) scope.
    use super::*;
    use crate::assert_epsilon;
    use crate::block_misc;
    use crate::feature_buffer;
    use block_helpers::slearn2;
    use block_misc::Observe;

    fn fb_vec() -> feature_buffer::FeatureBuffer {
        feature_buffer::FeatureBuffer {
            label: 0.0,
            example_importance: 1.0,
            example_number: 0,
            lr_buffer: Vec::new(),
            ffm_buffer: Vec::new(),
            ffm_fields_count: 0,
        }
    }

    #[test]
    fn test_simple_positive() {
        let mut mi = model_instance::ModelInstance::new_empty().unwrap();
        let mut bg = BlockGraph::new();
        let input_block = block_misc::new_const_block(&mut bg, vec![2.0]).unwrap();
        let relu_block = new_relu_block(&mut bg, &mi, input_block).unwrap();
        let observe_block =
            block_misc::new_observe_block(&mut bg, relu_block, Observe::Forward, Some(1.0))
                .unwrap();
        bg.finalize();
        bg.allocate_and_init_weights(&mi);

        let mut pb = bg.new_port_buffer();

        let fb = fb_vec();
        assert_epsilon!(slearn2(&mut bg, &fb, &mut pb, true), 2.0);
        assert_epsilon!(slearn2(&mut bg, &fb, &mut pb, true), 2.0); // relu desnt learn
    }
    fn test_simple_negative() {
        let mut mi = model_instance::ModelInstance::new_empty().unwrap();
        let mut bg = BlockGraph::new();
        let input_block = block_misc::new_const_block(&mut bg, vec![-2.0]).unwrap();
        let relu_block = new_relu_block(&mut bg, &mi, input_block).unwrap();
        let observe_block =
            block_misc::new_observe_block(&mut bg, relu_block, Observe::Forward, Some(1.0))
                .unwrap();
        bg.finalize();
        bg.allocate_and_init_weights(&mi);

        let mut pb = bg.new_port_buffer();

        let fb = fb_vec();
        assert_epsilon!(slearn2(&mut bg, &fb, &mut pb, true), 0.0);
        assert_epsilon!(slearn2(&mut bg, &fb, &mut pb, true), 0.0); // relu desnt learn
    }
}
