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

use std::slice;

use bytes::BufMut;
use futures::{io, AsyncRead, Stream};
use libc::{can_frame, canfd_frame};
use socketcan::frame::{can_frame_default, canfd_frame_default};
use socketcan::{CanAnyFrame, CanFdFrame, CanFrame, EmbeddedFrame};

const IMPLEMENTED_VERSION: u8 = 2;

#[allow(unused)]
enum OpCode {
    Data = 0x0,
    Ack = 0x1,
    Nack = 0x2,
}

pub struct MessageSerializer {
    sequence_number: u8,
    frames: Vec<u8>,
    count: u16,
}

pub fn encoded_size(frame: &CanAnyFrame) -> usize {
    match frame {
        CanAnyFrame::Normal(frame) => 4 + 1 + frame.data().len(),
        CanAnyFrame::Remote(frame) => 4 + 1 + frame.data().len(),
        CanAnyFrame::Error(frame) => 4 + 1 + frame.data().len(),
        CanAnyFrame::Fd(frame) => 4 + 1 + 1 + frame.data().len(),
    }
}

const CANFD_FRAME_MARKER: u8 = 0x80;

pub struct Finalizer<'serializer> {
    serializer: &'serializer mut MessageSerializer,
}

impl<'serializer> Finalizer<'serializer> {
    pub fn data(&self) -> &[u8] {
        &self.serializer.frames
    }
}

impl<'serializer> Drop for Finalizer<'serializer> {
    fn drop(&mut self) {
        self.serializer.reset()
    }
}

impl MessageSerializer {
    pub fn new() -> Self {
        let mut value = Self {
            sequence_number: 0,
            frames: Vec::new(),
            count: 0,
        };
        value.reset();
        value
    }

    #[allow(unused)]
    pub fn sequence_number(&self) -> u8 {
        self.sequence_number
    }

    fn serialize_2_0_frame<F: AsRef<can_frame>>(&mut self, frame: &F) {
        let frame = frame.as_ref();
        self.frames.put_u32(frame.can_id);
        self.frames.put_u8(frame.can_dlc);
        self.frames.put_slice(&frame.data[..frame.can_dlc as usize]);
    }

    fn serialize_fd_frame<F: AsRef<canfd_frame>>(&mut self, frame: &F) {
        let frame = frame.as_ref();
        self.frames.put_u32(frame.can_id);
        self.frames.put_u8(frame.len | CANFD_FRAME_MARKER);
        self.frames.put_u8(frame.flags);
        self.frames.put_slice(&frame.data[..frame.len as usize]);
    }

    pub fn push_frame(&mut self, frame: impl Into<socketcan::CanAnyFrame>) {
        match frame.into() {
            CanAnyFrame::Normal(frame) => self.serialize_2_0_frame(&frame),
            CanAnyFrame::Remote(frame) => self.serialize_2_0_frame(&frame),
            CanAnyFrame::Error(frame) => self.serialize_2_0_frame(&frame),
            CanAnyFrame::Fd(fd_frame) => self.serialize_fd_frame(&fd_frame),
        }
        self.count += 1
    }

    fn write_header(&mut self, length: u16) {
        self.frames.put_u8(IMPLEMENTED_VERSION);
        self.frames.put_u8(OpCode::Data as u8);
        self.frames.put_u8(self.sequence_number);
        self.frames.put_u16(length);

        self.sequence_number = self.sequence_number.wrapping_add(1);
    }

    pub fn reset(&mut self) {
        self.frames.clear();
        self.count = 0;
        self.write_header(
            0, /* placeholder that will be overwritten on finalize */
        )
    }

    pub fn finalize(&mut self) -> Finalizer {
        // overwrite length value
        self.frames[3..5].copy_from_slice(&self.count.to_be_bytes());

        Finalizer { serializer: self }
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }
}

pub async fn deserialize_from(mut buffer: impl AsyncRead + Unpin) -> anyhow::Result<CanAnyFrame> {
    use futures::AsyncReadExt;
    let id = {
        let mut bytes = [0u8; 4];
        buffer.read_exact(&mut bytes).await?;
        u32::from_be_bytes(bytes)
    };
    let mut len = 0u8;
    buffer.read_exact(slice::from_mut(&mut len)).await?;
    if len & CANFD_FRAME_MARKER > 0 {
        let mut frame: canfd_frame = canfd_frame_default();
        frame.can_id = id;
        frame.len = len & !CANFD_FRAME_MARKER;
        buffer.read_exact(slice::from_mut(&mut frame.flags)).await?;
        buffer
            .read_exact(&mut frame.data[..frame.len as usize])
            .await?;

        Ok(CanAnyFrame::Fd(CanFdFrame::try_from(frame)?))
    } else {
        let mut frame = can_frame_default();
        frame.can_id = id;
        frame.can_dlc = len;
        buffer
            .read_exact(&mut frame.data[..frame.can_dlc as usize])
            .await?;

        Ok(<CanAnyFrame as From<CanFrame>>::from(frame.into()))
    }
}

pub struct MessageReader<Buffer> {
    buffer: Buffer,
    remaining: u16,
    sequence_number: u8,
}

impl<Buffer: AsyncRead + Unpin> MessageReader<Buffer> {
    pub async fn try_read(mut buffer: Buffer) -> io::Result<Option<Self>> {
        // TODO: Range checks!

        use futures::AsyncReadExt;

        let mut version = 0u8;
        buffer.read_exact(slice::from_mut(&mut version)).await?;

        let mut op_code = 0u8;
        buffer.read_exact(slice::from_mut(&mut op_code)).await?;

        if IMPLEMENTED_VERSION == version && OpCode::Data as u8 == op_code {
            let mut sequence_number = 0u8;
            buffer
                .read_exact(slice::from_mut(&mut sequence_number))
                .await?;
            let remaining = {
                let mut bytes = [0u8; 2];
                buffer.read_exact(&mut bytes).await?;
                u16::from_be_bytes(bytes)
            };
            Ok(Some(Self {
                buffer,
                sequence_number,
                remaining,
            }))
        } else {
            Ok(None)
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

    pub fn into_stream(self) -> impl Stream<Item = anyhow::Result<CanAnyFrame>> {
        futures::stream::unfold(self, |mut this| async {
            if this.remaining > 0 {
                this.remaining -= 1;
                Some((deserialize_from(&mut this.buffer).await, this))
            } else {
                None
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use futures::pin_mut;
    use socketcan::{EmbeddedFrame, Id, StandardId};

    use super::*;

    #[test]
    fn empty() {
        smol::block_on(async {
            let mut serializer = MessageSerializer::new();
            for i in 0..10 {
                let result = serializer.finalize();
                let mut result = result.data();

                let deserializer = MessageReader::try_read(&mut result).await.unwrap().unwrap();

                assert_eq!(i, deserializer.sequence_number());
                assert_eq!(0, deserializer.remaining());
            }
        })
    }

    #[test]
    fn single_frame() {
        smol::block_on(async {
            let mut serializer = MessageSerializer::new();
            let frame = socketcan::CanFrame::new(
                Id::Standard(StandardId::new(0x14).unwrap()),
                &[0, 1, 2, 3, 4],
            )
            .unwrap();
            serializer.push_frame(frame);

            let result = serializer.finalize();
            let mut result = result.data();

            let deserialized = MessageReader::try_read(&mut result).await.unwrap().unwrap();

            assert_eq!(0, deserialized.sequence_number());
            assert_eq!(1, deserialized.remaining());

            use futures::TryStreamExt;
            // no PartialEq on socketcan::CANFrame :(
            let decoder = deserialized.into_stream();
            pin_mut!(decoder);
            let deserialized_frame = decoder
                .try_next()
                .await
                .expect("Frame must be deserializable!")
                .expect("Must contain at least one frame!");
            let CanAnyFrame::Normal(deserialized_frame) = deserialized_frame else {
                panic!("Got a different frame type than originally inserted into the stream!");
            };
            assert_eq!(frame.id(), deserialized_frame.id());
            assert_eq!(frame.data(), deserialized_frame.data());
        })
    }
}
