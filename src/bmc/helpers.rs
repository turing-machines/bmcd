// Copyright 2023 Turing Machines
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
const NODE_COUNT: u8 = 4;

/// small helper macro which handles the code duplication of declaring gpio lines.
#[macro_export]
macro_rules! gpio_output_lines {
    ($chip:ident, $output:expr) => {
        $chip
            .request_lines(gpiod::Options::output($output))
            .context(concat!("error initializing pin ", stringify!($output)))?
    };
}

/// uses [`gpio_output_lines`] to declare an array of `gpiod::Lines` objects
#[macro_export]
macro_rules! gpio_output_array {
    ($chip:ident, $($pin:ident),+) => {
       [
           $(
               $crate::gpio_output_lines!($chip, [$pin])
           ),*
       ]
    };
}

/// Helper function that converts a bitfield + mask into an iterator. This
/// iterator iterates over each bit, and skips the bits that are not set in the
/// nodes_mask.
///
/// # Arguments
///
/// * `node_states`     bit-field where each bit represents a node on the
/// turing-pi board, if bit(n) = 1 equals 'select' and bit(n) = 0 equals
/// 'unselect'.
/// * `node_mask`       mask which bits to select.
///
/// # Returns
///
/// iterator returns a tuple containing the index of a bit + the new value.  
pub fn bit_iterator(nodes_state: u8, nodes_mask: u8) -> impl Iterator<Item = (usize, u8)> {
    (0..NODE_COUNT).filter_map(move |n| {
        let mask = nodes_mask & (1 << n);
        let state = (nodes_state & mask) >> n;
        (mask != 0).then_some((n as usize, state))
    })
}
