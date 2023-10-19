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
mod event_listener;
mod io;
pub mod ring_buf;
#[doc(inline)]
pub use event_listener::*;
pub use io::*;

pub fn string_from_utf16(bytes: &[u8], little_endian: bool) -> String {
    let u16s = bytes.chunks_exact(2).map(|pair| {
        let Ok(owned) = pair.try_into() else {
            unreachable!()
        };

        if little_endian {
            u16::from_le_bytes(owned)
        } else {
            u16::from_be_bytes(owned)
        }
    });

    let mut string = char::decode_utf16(u16s)
        .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect::<String>();

    if bytes.len() % 2 == 1 {
        string.push(char::REPLACEMENT_CHARACTER)
    }

    string
}

pub fn string_from_utf32(bytes: &[u8], little_endian: bool) -> String {
    bytes
        .chunks(4)
        .map(|slice| {
            let Ok(owned) = slice.try_into() else {
                return char::REPLACEMENT_CHARACTER;
            };

            let scalar = if little_endian {
                u32::from_le_bytes(owned)
            } else {
                u32::from_be_bytes(owned)
            };

            char::from_u32(scalar).unwrap_or(char::REPLACEMENT_CHARACTER)
        })
        .collect()
}
