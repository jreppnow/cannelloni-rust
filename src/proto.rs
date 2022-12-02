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

const IMPLEMENTED_VERSION: u8 = 2;

#[allow(unused)]
enum OpCode {
    Data = 0x0,
    Ack = 0x1,
    Nack = 0x2,
}

pub struct MessageSerializer {
    sequence_number: u8,
    frames: Vec<socketcan::CANFrame>,
}

pub trait SerializeInto {
    fn serialize_into(&mut self, buffer: impl BufMut);

    fn serialize(&mut self) -> VecDeque<u8> {
        let mut buffer: Vec<u8> = Default::default();
        self.serialize_into(&mut buffer);
        <Vec<u8> as Into<VecDeque<u8>>>::into(buffer)
    }
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

    pub fn push_frame(&mut self, frame: socketcan::CANFrame) {
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
}

impl SerializeInto for socketcan::CANFrame {
    fn serialize_into(&mut self, mut buffer: impl BufMut) {
        buffer.put_u32({
            let mut id = self.id();

            if id & !socketcan::SFF_MASK > 0 {
                id |= socketcan::EFF_FLAG;
            }
            if self.is_error() {
                id |= socketcan::ERR_FLAG;
            }
            if self.is_rtr() {
                id |= socketcan::RTR_FLAG;
            }

            id
        });
        buffer.put_u8(self.data().len() as u8);
        // buffer.put_u8(self.flags) // TODO: CAN-FD
        buffer.put_slice(self.data());
    }
}

impl DeserializeFrom for socketcan::CANFrame {
    type Value = Self;
    type Error = socketcan::ConstructionError;

    fn deserialize_from(mut buffer: impl Buf) -> Result<Self::Value, Self::Error> {
        let id = buffer.get_u32();
        let len = buffer.get_u8() as usize;
        let mut data = [0u8; 8]; // Initialization unnecessary, but easier for now..
        buffer.copy_to_slice(&mut data[..(len)]);
        socketcan::CANFrame::new(id & socketcan::EFF_MASK /* TODO: This looks fishy.. */, &data[..len], id & socketcan::RTR_FLAG > 0, id & socketcan::ERR_FLAG > 0)
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

        if IMPLEMENTED_VERSION == buffer.get_u8() &&
            OpCode::Data as u8 == buffer.get_u8() {
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
    type Item = socketcan::CANFrame;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining > 0 {
            self.remaining -= 1;

            if let Ok(frame) = socketcan::CANFrame::deserialize_from(&mut self.buffer) {
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
        let frame = socketcan::CANFrame::new(0x14, &[0, 1, 2, 3, 4], true, true).unwrap();
        serializer.push_frame(frame);

        let mut result = serializer.serialize();

        let mut deserialized = MessageReader::try_read(&mut result).unwrap();

        assert_eq!(0, deserialized.sequence_number());
        assert_eq!(1, deserialized.remaining());

        // no PartialEq on socketcan::CANFrame :(
        let deserialized_frame = deserialized.next().expect("Frame must be deserializable!");
        assert_eq!(frame.id(), deserialized_frame.id());
        assert_eq!(frame.data(), deserialized_frame.data());
        assert_eq!(frame.is_error(), deserialized_frame.is_error());
        assert_eq!(frame.is_rtr(), deserialized_frame.is_rtr());
    }
}
