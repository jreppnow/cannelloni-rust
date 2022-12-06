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

#![feature(c_size_t)]
#![feature(future_join)] // for tests..

extern crate core;

mod async_can;
mod proto;

use crate::async_can::AsyncCanSocket;
use async_io::Timer;
use smol::net::{SocketAddr, UdpSocket};
use std::time::Duration;

use futures::prelude::*;

const UDP_NO_FRAGMENT_MAX_PAYLOAD_SIZE: usize = 508;

async fn send(
    can_socket: &AsyncCanSocket,
    udp_socket: &UdpSocket,
    force_after: Duration,
    target: &SocketAddr,
) {
    let mut encoder = MessageSerializer::new();
    let mut timer: Option<Fuse<Timer>> = None;

    loop {
        timer = match timer {
            None => {
                if let Ok(frame) = can_socket.read_frame().await {
                    if encoder.encoded_size() + frame.encoded_size()
                        > UDP_NO_FRAGMENT_MAX_PAYLOAD_SIZE
                    {
                        send_frame(udp_socket, &mut encoder, target).await;
                    }
                    encoder.push_frame(frame);
                    Some(futures::FutureExt::fuse(Timer::after(force_after)))
                } else {
                    None
                }
            }
            Some(mut timer) => {
                select! {
                    _ = &mut timer => {
                        send_frame(udp_socket, &mut encoder, target).await;
                        None
                    }
                    frame = can_socket.read_frame().fuse() => {
                        if let Ok(frame) = frame {
                            if encoder.encoded_size() + frame.encoded_size() > UDP_NO_FRAGMENT_MAX_PAYLOAD_SIZE {
                                send_frame(udp_socket, &mut encoder, target).await;
                                encoder.push_frame(frame);
                                // new timer, since we have sent the last packet..
                                Some(futures::FutureExt::fuse(Timer::after(force_after)))
                            } else {
                                encoder.push_frame(frame);
                                Some(timer)
                            }
                        } else {
                            Some(timer)
                        }
                    }
                }
            }
        }
    }
}

async fn send_frame(udp_socket: &UdpSocket, encoder: &mut MessageSerializer, target: &SocketAddr) {
    let mut serialized = encoder.serialize();
    if let Err(err) = udp_socket
        .send_to(serialized.make_contiguous(), target)
        .await
    {
        eprintln!("Failed to send via UDP socket! {}", err);
    };
}

/// Listen for packets on a UDP socket, unwrap them and transfer them onto a can socket..
///
/// # Arguments
///
/// * `can_socket`: The CAN socket (destination).
/// * `udp_socket`:  The UDP socket (source).
/// * `local_address`: The local address of this applications. If the sender address of a packet is the same as this, it will be ignored. Needed for multicast.
///
/// returns: ()
async fn receive(
    can_socket: &AsyncCanSocket,
    udp_socket: &UdpSocket,
    local_address: SocketAddr, /* TODO: Make this an Option? Unfortunately does not play nice with match or if let.. */
) {
    let mut buffer = [0u8; UDP_NO_FRAGMENT_MAX_PAYLOAD_SIZE];
    loop {
        match udp_socket.recv_from(&mut buffer).await {
            Ok((n, peer)) => {
                if peer != local_address {
                    if let Some(decoder) = proto::MessageReader::try_read(&buffer[..n]) {
                        for frame in decoder {
                            let _ = can_socket.write_frame(&frame).await;
                        }
                    }
                }
            }
            Err(err) => eprintln!("Failed to read from UDP socket! {}", err),
        }
    }
}

use crate::future::Fuse;
use crate::proto::{MessageSerializer, SerializeInto};
use clap::Parser;
use futures::select;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Local address to bind to.
    #[arg(long, short)]
    bind: SocketAddr,

    /// Remote address to connect to.
    #[arg(long, short)]
    remote: SocketAddr,

    /// CAN interface name to forward frames to and from.
    #[arg(long, short)]
    can: String,

    /// After how many milliseconds a packet must be sent, regardless of how many frames it contains.
    #[arg(long, short, default_value_t = 50)]
    force_after_ms: usize,
}

fn main() {
    let args = Args::parse();

    smol::block_on(async {
        let (udp_sender, udp_receiver) = create_udp_sockets(&args.bind, &args.remote).await;

        let can_socket: AsyncCanSocket = socketcan::CANSocket::open(&args.can).unwrap().into();

        select! {
            _ = send(&can_socket, &udp_sender, Duration::from_millis(args.force_after_ms as u64), &args.remote).fuse() => (),
            _ = receive(&can_socket, &udp_receiver, udp_sender.local_addr().expect("Failed to get local addr!")).fuse() => (),
        }
    });
}

async fn create_udp_sockets(local: &SocketAddr, remote: &SocketAddr) -> (UdpSocket, UdpSocket) {
    match (remote, local) {
        (SocketAddr::V4(remote), SocketAddr::V4(local)) if remote.ip().is_multicast() => {
            let udp_receiver = UdpSocket::bind(remote)
                .await
                .expect("Failed to bind to multicast address.");
            udp_receiver
                .join_multicast_v4(*remote.ip(), *local.ip())
                .expect("Failed to join multicast-group!");

            let udp_sender = UdpSocket::bind(local)
                .await
                .expect("Failed to bind to local address.");
            (udp_sender, udp_receiver)
        }
        (SocketAddr::V6(remote), SocketAddr::V6(local)) if remote.ip().is_multicast() => {
            let udp_receiver = UdpSocket::bind(remote)
                .await
                .expect("Failed to bind to multicast address.");
            udp_receiver
                .join_multicast_v6(remote.ip(), remote.scope_id())
                .expect("Failed to join multicast-group!");

            let udp_sender = UdpSocket::bind(local)
                .await
                .expect("Failed to bind to local address.");
            (udp_sender, udp_receiver)
        }
        (SocketAddr::V4(_), SocketAddr::V4(_)) | (SocketAddr::V6(_), SocketAddr::V6(_)) => {
            let udp_socket = bind_simple(local, remote).await;
            (udp_socket.clone(), udp_socket)
        }
        _ => panic!("Must specify either IPv4 or IPv6 for both bind and connect addresses!"),
    }
}

async fn bind_simple(local_address: &SocketAddr, remote_address: &SocketAddr) -> UdpSocket {
    let udp_socket = UdpSocket::bind(local_address)
        .await
        .expect("Failed to bind to local address.");
    udp_socket
        .connect(remote_address)
        .await
        .expect("Failed to connect to remote address");
    udp_socket
}
