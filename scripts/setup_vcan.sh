#! /bin/sh

ip link add dev $1 type vcan
ip link set $1 mtu 72 # CAN-FD
ip link set up $1
