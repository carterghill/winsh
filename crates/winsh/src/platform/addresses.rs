//! Read the addresses assigned to active Windows network adapters.
#![allow(unsafe_code)]

use std::{
    collections::BTreeSet,
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    ptr,
};

use windows_sys::Win32::{
    Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_NO_DATA, NO_ERROR},
    NetworkManagement::{
        IpHelper::{
            GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
            GetAdaptersAddresses, IF_TYPE_SOFTWARE_LOOPBACK, IP_ADAPTER_ADDRESSES_LH,
        },
        Ndis::IfOperStatusUp,
    },
    Networking::WinSock::{
        AF_INET, AF_INET6, AF_UNSPEC, IpDadStateDeprecated, IpDadStatePreferred, SOCKADDR_IN,
        SOCKADDR_IN6,
    },
};

pub(crate) fn local() -> io::Result<BTreeSet<IpAddr>> {
    let mut result = BTreeSet::new();
    let mut size = 15 * 1024; // Recommended initial size for GetAdaptersAddresses.
    loop {
        // Allocate in units of the adapter struct for native alignment. Windows
        // fills this buffer with adapters, address nodes, and their socket addresses.
        let count = (size as usize).div_ceil(size_of::<IP_ADAPTER_ADDRESSES_LH>());
        let mut buffer = Vec::<IP_ADAPTER_ADDRESSES_LH>::with_capacity(count);
        let mut adapter = buffer.as_mut_ptr();
        // SAFETY: the allocation is writable for at least size bytes and stays
        // alive and unmoved until both linked lists have been traversed.
        let status = unsafe {
            GetAdaptersAddresses(
                AF_UNSPEC.into(),
                GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER,
                ptr::null(),
                adapter,
                &mut size,
            )
        };
        match status {
            ERROR_BUFFER_OVERFLOW => continue,
            ERROR_NO_DATA => return Ok(result),
            NO_ERROR => {}
            code => return Err(io::Error::from_raw_os_error(code as i32)),
        }
        // SAFETY: on success Windows initializes both null-terminated lists.
        // Each socket address has the layout indicated by its address family.
        unsafe {
            while let Some(current) = adapter.as_ref() {
                adapter = current.Next;
                if current.OperStatus != IfOperStatusUp
                    || current.IfType == IF_TYPE_SOFTWARE_LOOPBACK
                {
                    continue;
                }
                let mut address = current.FirstUnicastAddress;
                while let Some(entry) = address.as_ref() {
                    address = entry.Next;
                    if entry.DadState != IpDadStatePreferred
                        && entry.DadState != IpDadStateDeprecated
                    {
                        continue;
                    }
                    let socket = entry.Address.lpSockaddr;
                    let ip = match (*socket).sa_family {
                        AF_INET => {
                            let raw = (*socket.cast::<SOCKADDR_IN>()).sin_addr.S_un.S_addr;
                            IpAddr::V4(Ipv4Addr::from(raw.to_ne_bytes()))
                        }
                        AF_INET6 => {
                            let raw = (*socket.cast::<SOCKADDR_IN6>()).sin6_addr.u.Byte;
                            let ip = Ipv6Addr::from(raw);
                            if ip.is_unicast_link_local() {
                                continue;
                            }
                            IpAddr::V6(ip)
                        }
                        _ => continue,
                    };
                    if !ip.is_loopback() && !ip.is_unspecified() {
                        result.insert(ip);
                    }
                }
            }
        }
        return Ok(result);
    }
}
