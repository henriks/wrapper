use crate::GuestNetwork;

const ETHERTYPE_ARP: u16 = 0x0806;
const ETHERTYPE_IPV4: u16 = 0x0800;
const ARP_HTYPE_ETHERNET: u16 = 1;
const ARP_PTYPE_IPV4: u16 = 0x0800;
const ARP_REQUEST: u16 = 1;
const ARP_REPLY: u16 = 2;
const IPPROTO_UDP: u8 = 17;
const DHCP_SERVER_PORT: u16 = 67;
const DHCP_CLIENT_PORT: u16 = 68;
const DHCP_MAGIC_COOKIE: [u8; 4] = [99, 130, 83, 99];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct L2Gateway {
    pub gateway_mac: [u8; 6],
    pub guest_mac: [u8; 6],
    pub gateway_ip: [u8; 4],
    pub guest_ip: [u8; 4],
    pub dns_ip: [u8; 4],
    pub prefix_len: u8,
    pub mtu: u16,
}

impl L2Gateway {
    pub fn from_guest_network(
        network: &GuestNetwork,
        gateway_mac: [u8; 6],
        mtu: u16,
    ) -> Result<Self, ParseAddressError> {
        Ok(Self {
            gateway_mac,
            guest_mac: parse_mac(&network.guest_mac)?,
            gateway_ip: parse_ipv4(&network.gateway_ip)?,
            guest_ip: parse_ipv4(&network.guest_ip)?,
            dns_ip: parse_ipv4(&network.dns_ip)?,
            prefix_len: network.prefix_len,
            mtu,
        })
    }

    pub fn handle_frame(&self, frame: &[u8]) -> Option<Vec<u8>> {
        if frame.len() < 14 {
            return None;
        }
        let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
        match ethertype {
            ETHERTYPE_ARP => self.handle_arp(frame),
            ETHERTYPE_IPV4 => self.handle_ipv4(frame),
            _ => None,
        }
    }

    fn handle_arp(&self, frame: &[u8]) -> Option<Vec<u8>> {
        if frame.len() < 42 {
            return None;
        }
        let arp = &frame[14..42];
        let htype = u16::from_be_bytes([arp[0], arp[1]]);
        let ptype = u16::from_be_bytes([arp[2], arp[3]]);
        let hlen = arp[4];
        let plen = arp[5];
        let oper = u16::from_be_bytes([arp[6], arp[7]]);
        if htype != ARP_HTYPE_ETHERNET
            || ptype != ARP_PTYPE_IPV4
            || hlen != 6
            || plen != 4
            || oper != ARP_REQUEST
            || arp[24..28] != self.gateway_ip
        {
            return None;
        }
        let sender_mac: [u8; 6] = arp[8..14].try_into().ok()?;
        let sender_ip: [u8; 4] = arp[14..18].try_into().ok()?;

        let mut reply = Vec::with_capacity(42);
        reply.extend_from_slice(&sender_mac);
        reply.extend_from_slice(&self.gateway_mac);
        reply.extend_from_slice(&ETHERTYPE_ARP.to_be_bytes());
        reply.extend_from_slice(&ARP_HTYPE_ETHERNET.to_be_bytes());
        reply.extend_from_slice(&ARP_PTYPE_IPV4.to_be_bytes());
        reply.push(6);
        reply.push(4);
        reply.extend_from_slice(&ARP_REPLY.to_be_bytes());
        reply.extend_from_slice(&self.gateway_mac);
        reply.extend_from_slice(&self.gateway_ip);
        reply.extend_from_slice(&sender_mac);
        reply.extend_from_slice(&sender_ip);
        Some(reply)
    }

    fn handle_ipv4(&self, frame: &[u8]) -> Option<Vec<u8>> {
        if frame.len() < 34 {
            return None;
        }
        let ip = &frame[14..];
        let ihl = usize::from(ip[0] & 0x0f) * 4;
        if ip[0] >> 4 != 4 || ihl < 20 || ip.len() < ihl + 8 || ip[9] != IPPROTO_UDP {
            return None;
        }
        let total_len = usize::from(u16::from_be_bytes([ip[2], ip[3]]));
        if total_len < ihl + 8 || ip.len() < total_len {
            return None;
        }
        let udp = &ip[ihl..total_len];
        let src_port = u16::from_be_bytes([udp[0], udp[1]]);
        let dst_port = u16::from_be_bytes([udp[2], udp[3]]);
        let udp_len = usize::from(u16::from_be_bytes([udp[4], udp[5]]));
        if src_port != DHCP_CLIENT_PORT
            || dst_port != DHCP_SERVER_PORT
            || udp_len < 8
            || udp.len() < udp_len
        {
            return None;
        }
        self.handle_dhcp(&udp[8..udp_len])
    }

    fn handle_dhcp(&self, payload: &[u8]) -> Option<Vec<u8>> {
        if payload.len() < 240 || payload[236..240] != DHCP_MAGIC_COOKIE {
            return None;
        }
        let message_type = dhcp_option(payload, 53).and_then(|value| value.first().copied())?;
        let response_type = match message_type {
            1 => 2,
            3 => 5,
            _ => return None,
        };
        let client_mac: [u8; 6] = payload[28..34].try_into().ok()?;
        let xid: [u8; 4] = payload[4..8].try_into().ok()?;
        Some(self.dhcp_response(response_type, xid, client_mac))
    }

    fn dhcp_response(&self, message_type: u8, xid: [u8; 4], client_mac: [u8; 6]) -> Vec<u8> {
        let mut bootp = vec![0; 240];
        bootp[0] = 2;
        bootp[1] = 1;
        bootp[2] = 6;
        bootp[4..8].copy_from_slice(&xid);
        bootp[16..20].copy_from_slice(&self.guest_ip);
        bootp[20..24].copy_from_slice(&self.gateway_ip);
        bootp[28..34].copy_from_slice(&client_mac);
        bootp[236..240].copy_from_slice(&DHCP_MAGIC_COOKIE);
        bootp.extend_from_slice(&[53, 1, message_type]);
        bootp.extend_from_slice(&[54, 4]);
        bootp.extend_from_slice(&self.gateway_ip);
        bootp.extend_from_slice(&[51, 4, 0, 0, 14, 16]);
        bootp.extend_from_slice(&[1, 4]);
        bootp.extend_from_slice(&subnet_mask(self.prefix_len));
        bootp.extend_from_slice(&[3, 4]);
        bootp.extend_from_slice(&self.gateway_ip);
        bootp.extend_from_slice(&[6, 4]);
        bootp.extend_from_slice(&self.dns_ip);
        bootp.extend_from_slice(&[26, 2]);
        bootp.extend_from_slice(&self.mtu.to_be_bytes());
        bootp.push(255);

        let udp_len = 8 + bootp.len();
        let ip_total_len = 20 + udp_len;
        let mut ipv4 = Vec::with_capacity(ip_total_len);
        ipv4.push(0x45);
        ipv4.push(0);
        ipv4.extend_from_slice(&(ip_total_len as u16).to_be_bytes());
        ipv4.extend_from_slice(&0_u16.to_be_bytes());
        ipv4.extend_from_slice(&0_u16.to_be_bytes());
        ipv4.push(64);
        ipv4.push(IPPROTO_UDP);
        ipv4.extend_from_slice(&0_u16.to_be_bytes());
        ipv4.extend_from_slice(&self.gateway_ip);
        ipv4.extend_from_slice(&[255, 255, 255, 255]);
        let checksum = ipv4_checksum(&ipv4);
        ipv4[10..12].copy_from_slice(&checksum.to_be_bytes());
        ipv4.extend_from_slice(&DHCP_SERVER_PORT.to_be_bytes());
        ipv4.extend_from_slice(&DHCP_CLIENT_PORT.to_be_bytes());
        ipv4.extend_from_slice(&(udp_len as u16).to_be_bytes());
        ipv4.extend_from_slice(&0_u16.to_be_bytes());
        ipv4.extend_from_slice(&bootp);

        let mut frame = Vec::with_capacity(14 + ipv4.len());
        frame.extend_from_slice(&[0xff; 6]);
        frame.extend_from_slice(&self.gateway_mac);
        frame.extend_from_slice(&ETHERTYPE_IPV4.to_be_bytes());
        frame.extend_from_slice(&ipv4);
        frame
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseAddressError {
    InvalidIpv4,
    InvalidMac,
}

fn dhcp_option(payload: &[u8], code: u8) -> Option<&[u8]> {
    let mut index = 240;
    while index < payload.len() {
        match payload[index] {
            0 => index += 1,
            255 => return None,
            current => {
                let len = *payload.get(index + 1)? as usize;
                let start = index + 2;
                let end = start + len;
                if end > payload.len() {
                    return None;
                }
                if current == code {
                    return Some(&payload[start..end]);
                }
                index = end;
            }
        }
    }
    None
}

fn parse_ipv4(value: &str) -> Result<[u8; 4], ParseAddressError> {
    let parts: Vec<_> = value.split('.').collect();
    if parts.len() != 4 {
        return Err(ParseAddressError::InvalidIpv4);
    }
    let mut bytes = [0; 4];
    for (index, part) in parts.iter().enumerate() {
        bytes[index] = part
            .parse::<u8>()
            .map_err(|_| ParseAddressError::InvalidIpv4)?;
    }
    Ok(bytes)
}

fn parse_mac(value: &str) -> Result<[u8; 6], ParseAddressError> {
    let parts: Vec<_> = value.split(':').collect();
    if parts.len() != 6 {
        return Err(ParseAddressError::InvalidMac);
    }
    let mut bytes = [0; 6];
    for (index, part) in parts.iter().enumerate() {
        if part.len() != 2 {
            return Err(ParseAddressError::InvalidMac);
        }
        bytes[index] = u8::from_str_radix(part, 16).map_err(|_| ParseAddressError::InvalidMac)?;
    }
    Ok(bytes)
}

fn subnet_mask(prefix_len: u8) -> [u8; 4] {
    let mask = if prefix_len == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(prefix_len.min(32)))
    };
    mask.to_be_bytes()
}

pub(crate) fn ipv4_checksum(header: &[u8]) -> u16 {
    let mut sum = 0_u32;
    for chunk in header.chunks(2) {
        let word = if chunk.len() == 2 {
            u16::from_be_bytes([chunk[0], chunk[1]]) as u32
        } else {
            (chunk[0] as u32) << 8
        };
        sum = sum.wrapping_add(word);
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gateway() -> L2Gateway {
        L2Gateway::from_guest_network(&GuestNetwork::default(), [0x02, 0xfc, 0, 0, 0, 1], 1500)
            .expect("gateway")
    }

    #[test]
    fn replies_to_arp_for_gateway_ip() {
        let gateway = gateway();
        let guest_mac = [0x02, 0xfc, 0x12, 0x34, 0x56, 0x78];
        let mut request = Vec::new();
        request.extend_from_slice(&[0xff; 6]);
        request.extend_from_slice(&guest_mac);
        request.extend_from_slice(&ETHERTYPE_ARP.to_be_bytes());
        request.extend_from_slice(&ARP_HTYPE_ETHERNET.to_be_bytes());
        request.extend_from_slice(&ARP_PTYPE_IPV4.to_be_bytes());
        request.push(6);
        request.push(4);
        request.extend_from_slice(&ARP_REQUEST.to_be_bytes());
        request.extend_from_slice(&guest_mac);
        request.extend_from_slice(&[10, 0, 2, 15]);
        request.extend_from_slice(&[0; 6]);
        request.extend_from_slice(&[10, 0, 2, 2]);

        let reply = gateway.handle_frame(&request).expect("arp reply");

        assert_eq!(&reply[0..6], &guest_mac);
        assert_eq!(&reply[6..12], &gateway.gateway_mac);
        assert_eq!(
            u16::from_be_bytes(reply[20..22].try_into().unwrap()),
            ARP_REPLY
        );
        assert_eq!(&reply[22..28], &gateway.gateway_mac);
        assert_eq!(&reply[28..32], &[10, 0, 2, 2]);
        assert_eq!(&reply[32..38], &guest_mac);
        assert_eq!(&reply[38..42], &[10, 0, 2, 15]);
    }

    #[test]
    fn ignores_arp_for_non_gateway_ip_and_malformed_frames() {
        let gateway = gateway();
        let guest_mac = [0x02, 0xfc, 0x12, 0x34, 0x56, 0x78];
        let mut request = Vec::new();
        request.extend_from_slice(&[0xff; 6]);
        request.extend_from_slice(&guest_mac);
        request.extend_from_slice(&ETHERTYPE_ARP.to_be_bytes());
        request.extend_from_slice(&ARP_HTYPE_ETHERNET.to_be_bytes());
        request.extend_from_slice(&ARP_PTYPE_IPV4.to_be_bytes());
        request.push(6);
        request.push(4);
        request.extend_from_slice(&ARP_REQUEST.to_be_bytes());
        request.extend_from_slice(&guest_mac);
        request.extend_from_slice(&[10, 0, 2, 15]);
        request.extend_from_slice(&[0; 6]);
        request.extend_from_slice(&[10, 0, 2, 99]);

        assert_eq!(gateway.handle_frame(&request), None);
        assert_eq!(gateway.handle_frame(&request[..20]), None);
    }

    #[test]
    fn offers_fixed_dhcp_lease() {
        let gateway = gateway();
        let discover = dhcp_discover();

        let reply = gateway.handle_frame(&discover).expect("dhcp offer");

        assert_eq!(
            u16::from_be_bytes(reply[12..14].try_into().unwrap()),
            ETHERTYPE_IPV4
        );
        assert_eq!(&reply[26..30], &[10, 0, 2, 2]);
        assert_eq!(&reply[30..34], &[255, 255, 255, 255]);
        let bootp = &reply[42..];
        assert_eq!(bootp[0], 2);
        assert_eq!(&bootp[16..20], &[10, 0, 2, 15]);
        assert_eq!(dhcp_option(bootp, 53), Some(&[2][..]));
        assert_eq!(dhcp_option(bootp, 54), Some(&[10, 0, 2, 2][..]));
        assert_eq!(dhcp_option(bootp, 6), Some(&[10, 0, 2, 3][..]));
    }

    #[test]
    fn ignores_truncated_or_non_dhcp_udp_frames() {
        let gateway = gateway();
        let discover = dhcp_discover();
        let mut wrong_port = discover.clone();
        wrong_port[36..38].copy_from_slice(&1234_u16.to_be_bytes());

        assert_eq!(gateway.handle_frame(&discover[..30]), None);
        assert_eq!(gateway.handle_frame(&wrong_port), None);
    }

    fn dhcp_discover() -> Vec<u8> {
        let guest_mac = [0x02, 0xfc, 0x12, 0x34, 0x56, 0x78];
        let mut bootp = vec![0; 240];
        bootp[0] = 1;
        bootp[1] = 1;
        bootp[2] = 6;
        bootp[4..8].copy_from_slice(&[0x12, 0x34, 0x56, 0x78]);
        bootp[28..34].copy_from_slice(&guest_mac);
        bootp[236..240].copy_from_slice(&DHCP_MAGIC_COOKIE);
        bootp.extend_from_slice(&[53, 1, 1, 255]);

        let udp_len = 8 + bootp.len();
        let ip_total_len = 20 + udp_len;
        let mut ipv4 = Vec::new();
        ipv4.push(0x45);
        ipv4.push(0);
        ipv4.extend_from_slice(&(ip_total_len as u16).to_be_bytes());
        ipv4.extend_from_slice(&0_u16.to_be_bytes());
        ipv4.extend_from_slice(&0_u16.to_be_bytes());
        ipv4.push(64);
        ipv4.push(IPPROTO_UDP);
        ipv4.extend_from_slice(&0_u16.to_be_bytes());
        ipv4.extend_from_slice(&[0, 0, 0, 0]);
        ipv4.extend_from_slice(&[255, 255, 255, 255]);
        let checksum = ipv4_checksum(&ipv4);
        ipv4[10..12].copy_from_slice(&checksum.to_be_bytes());
        ipv4.extend_from_slice(&DHCP_CLIENT_PORT.to_be_bytes());
        ipv4.extend_from_slice(&DHCP_SERVER_PORT.to_be_bytes());
        ipv4.extend_from_slice(&(udp_len as u16).to_be_bytes());
        ipv4.extend_from_slice(&0_u16.to_be_bytes());
        ipv4.extend_from_slice(&bootp);

        let mut frame = Vec::new();
        frame.extend_from_slice(&[0xff; 6]);
        frame.extend_from_slice(&guest_mac);
        frame.extend_from_slice(&ETHERTYPE_IPV4.to_be_bytes());
        frame.extend_from_slice(&ipv4);
        frame
    }
}
