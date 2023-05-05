/*
 *  Copyright (c) 2022 Janosch Reppnow.
 *
 *  This program is free software; you can redistribute it and/or
 *  modify it under the terms of the GNU Lesser General Public
 *  License as published by the Free Software Foundation; either
 *  version 3 of the License, or (at your option) any later version.
 *
 *  This program is distributed in the hope that it will be useful,
 *  but WITHOUT ANY WARRANTY; without even the implied warranty of
 *  MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
 *  Lesser General Public License for more details.
 *
 *  You should have received a copy of the GNU Lesser General Public License
 *  along with this program; if not, write to the Free Software Foundation,
 *  Inc., 51 Franklin Street, Fifth Floor, Boston, MA  02110-1301, USA.
 */

use std::collections::VecDeque;

use bytes::{Buf, BufMut};
use socketcan::frame::{can_frame_default, AsPtr};
use socketcan::EmbeddedFrame;

const IMPLEMENTED_VERSION: u8 = 2;

#[allow(unused)]
enum OpCode {
    Data = 0x0,
    Ack = 0x1,
    Nack = 0x2,
}

pub struct MessageSerializer {
    sequence_number: u8,
    frames: Vec<socketcan::CanFrame>,
}

pub trait SerializeInto {
    fn serialize_into(&mut self, buffer: impl BufMut);

    fn serialize(&mut self) -> VecDeque<u8> {
        let mut buffer: Vec<u8> = Default::default();
        self.serialize_into(&mut buffer);
        <Vec<u8> as Into<VecDeque<u8>>>::into(buffer)
    }

    fn encoded_size(&self) -> usize;
}

trait DeserializeFrom {
    type Value;
    type Error;

    fn deserialize_from(buffer: impl Buf) -> Result<Self::Value, Self::Error>;
}

impl MessageSerializer {
    pub fn new() -> Self {
        Self {
            sequence_number: 0,
            frames: Vec::new(),
        }
    }

    #[allow(unused)]
    pub fn sequence_number(&self) -> u8 {
        self.sequence_number
    }

    pub fn push_frame(&mut self, frame: socketcan::CanFrame) {
        self.frames.push(frame)
    }
}

impl SerializeInto for MessageSerializer {
    fn serialize_into(&mut self, mut buffer: impl BufMut) {
        // TODO: Check for available len?
        buffer.put_u8(IMPLEMENTED_VERSION);
        buffer.put_u8(OpCode::Data as u8);
        buffer.put_u8(self.sequence_number);
        buffer.put_u16(self.frames.len() as u16);

        for mut frame in self.frames.drain(..) {
            frame.serialize_into(&mut buffer);
        }

        self.sequence_number = self.sequence_number.wrapping_add(1);
    }

    fn encoded_size(&self) -> usize {
        let mut size = 5;
        for frame in &self.frames {
            size += frame.encoded_size();
        }
        size
    }
}

impl SerializeInto for socketcan::CanFrame {
    fn serialize_into(&mut self, mut buffer: impl BufMut) {
        let self_raw = unsafe { &*self.as_ptr() };

        buffer.put_u32(self_raw.can_id);
        buffer.put_u8(self_raw.can_dlc);
        // buffer.put_u8(self.flags) // TODO: CAN-FD
        buffer.put_slice(&self_raw.data[..self_raw.can_dlc as usize]);
    }

    fn encoded_size(&self) -> usize {
        4 + 1 + self.data().len()
    }
}

impl DeserializeFrom for socketcan::CanFrame {
    type Value = Self;
    type Error = socketcan::ConstructionError;

    fn deserialize_from(
        mut buffer: impl Buf,
    ) -> Result<Self::Value, <socketcan::CanFrame as DeserializeFrom>::Error> {
        let id = buffer.get_u32();
        let len = buffer.get_u8();
        let mut data = [0u8; 8]; // Initialization unnecessary, but easier for now..
        buffer.copy_to_slice(&mut data[..len as usize]);
        let mut frame = can_frame_default();
        frame.can_id = id;
        frame.data = data;
        frame.can_dlc = len;
        Ok(frame.into())
    }
}

pub struct MessageReader<Buffer> {
    buffer: Buffer,
    remaining: u16,
    sequence_number: u8,
}

impl<Buffer: Buf> MessageReader<Buffer> {
    pub fn try_read(mut buffer: Buffer) -> Option<Self> {
        // TODO: Range checks!

        if IMPLEMENTED_VERSION == buffer.get_u8() && OpCode::Data as u8 == buffer.get_u8() {
            let sequence_number = buffer.get_u8();
            let remaining = buffer.get_u16();
            Some(Self {
                buffer,
                sequence_number,
                remaining,
            })
        } else {
            None
        }
    }

    #[allow(unused)]
    pub fn remaining(&self) -> u16 {
        self.remaining
    }

    #[allow(unused)]
    pub fn sequence_number(&self) -> u8 {
        self.sequence_number
    }
}

impl<Buffer: Buf> Iterator for MessageReader<Buffer> {
    type Item = socketcan::CanFrame;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining > 0 {
            self.remaining -= 1;

            if let Ok(frame) = socketcan::CanFrame::deserialize_from(&mut self.buffer) {
                return Some(frame);
            } else {
                self.remaining = 0;
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use socketcan::CanFrame::{Error, Remote};
    use socketcan::{EmbeddedFrame, Id, StandardId};

    use super::*;

    #[test]
    fn empty() {
        let mut serializer = MessageSerializer::new();
        for i in 0..10 {
            let mut result = serializer.serialize();

            let deserialized = MessageReader::try_read(&mut result).unwrap();

            assert_eq!(i, deserialized.sequence_number());
            assert_eq!(0, deserialized.remaining());
        }
    }

    #[test]
    fn single_frame() {
        let mut serializer = MessageSerializer::new();
        let frame = socketcan::CanFrame::new(
            Id::Standard(StandardId::new(0x14).unwrap()),
            &[0, 1, 2, 3, 4],
        )
        .unwrap();
        serializer.push_frame(frame);

        let mut result = serializer.serialize();

        let mut deserialized = MessageReader::try_read(&mut result).unwrap();

        assert_eq!(0, deserialized.sequence_number());
        assert_eq!(1, deserialized.remaining());

        // no PartialEq on socketcan::CANFrame :(
        let deserialized_frame = deserialized.next().expect("Frame must be deserializable!");
        assert_eq!(frame.id(), deserialized_frame.id());
        assert_eq!(frame.data(), deserialized_frame.data());
        if let Error(_) | Remote(_) = deserialized_frame {
            panic!("Should be a data frame!");
        }
    }
}
