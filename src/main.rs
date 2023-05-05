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

use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use futures::prelude::*;
use futures::select;
use smol::net::SocketAddr;
use socketcan::Socket;

use crate::async_can::AsyncCanSocket;

mod async_can;
mod proto;
mod udp;

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

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    smol::block_on(async {
        let (udp_sender, udp_receiver) = udp::create_udp_sockets(&args.bind, &args.remote).await.context("Creating UDP sockets..")?;

        let can_socket: AsyncCanSocket = socketcan::CanSocket::open(&args.can).context("Trying to open async can socket..").context("Creating CAN socket..")?.into();

        select! {
            sent = udp::send(&can_socket, &udp_sender, Duration::from_millis(args.force_after_ms as u64), &args.remote).fuse() => sent.context("Trying to send frames to the remote.."),
            received = udp::receive(&can_socket, &udp_receiver, udp_sender.local_addr().context("Getting local addr from socket..")?).fuse() => received.context("Trying to receive frames from the remote.."),
        }
    }).context("Asynchronously handling sending and receiving..")?;

    Ok(())
}
