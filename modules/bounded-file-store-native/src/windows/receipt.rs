use std::{
    mem::{offset_of, size_of},
    slice,
};

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};
use windows_sys::{
    Wdk::Storage::FileSystem::{FILE_FULL_EA_INFORMATION, NtQueryEaFile, NtSetEaFile},
    Win32::{
        Foundation::{HANDLE, STATUS_NO_EAS_ON_FILE, STATUS_NONEXISTENT_EA_ENTRY},
        System::IO::IO_STATUS_BLOCK,
    },
};

const EA_NAME: &[u8] = b"ARTISAN.RECEIPT.V1";
const FORMAT: u8 = 1;
const MAC_LENGTH: usize = 32;
const PAYLOAD_LENGTH: usize = 1 + 1 + 32 + 32 + 32 + 8 + 16 + 8 + 16;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ReceiptRole {
    CreatingStage = 0,
    Stage = 1,
    Backup = 2,
    Finalizing = 3,
    Restoring = 4,
}

impl ReceiptRole {
    fn from_byte(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::CreatingStage),
            1 => Some(Self::Stage),
            2 => Some(Self::Backup),
            3 => Some(Self::Finalizing),
            4 => Some(Self::Restoring),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Marker {
    pub(super) role: ReceiptRole,
    pub(super) namespace: [u8; 32],
    pub(super) expected: [u8; 32],
    pub(super) replacement: [u8; 32],
    pub(super) self_volume: u64,
    pub(super) self_id: [u8; 16],
    pub(super) peer_volume: u64,
    pub(super) peer_id: [u8; 16],
}

pub(super) struct EaBuffer {
    storage: Vec<u32>,
    length: usize,
}

impl EaBuffer {
    pub(super) fn as_bytes(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self.storage.as_ptr() as *const u8, self.length) }
    }

    fn as_mut_bytes(&mut self) -> &mut [u8] {
        unsafe { slice::from_raw_parts_mut(self.storage.as_mut_ptr() as *mut u8, self.length) }
    }
}

pub(super) fn derive_root_key(master_key: &[u8; 32], volume: u64, id: [u8; 16]) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(master_key).expect("HMAC accepts fixed-size keys");

    mac.update(b"ARTISAN.BOUNDED.FILE.STORE.ROOT.V1");
    mac.update(&volume.to_le_bytes());
    mac.update(&id);

    mac.finalize().into_bytes().into()
}

pub(super) fn namespace(operation_id: &str, path: &str) -> [u8; 32] {
    let canonical_path = path.replace('\\', "/").to_lowercase();
    let mut digest = Sha256::new();

    digest.update(operation_id.as_bytes());
    digest.update([0]);
    digest.update(canonical_path.as_bytes());

    digest.finalize().into()
}

pub(super) fn hash(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

pub(super) fn read(handle: HANDLE, key: &[u8; 32]) -> Result<Option<Marker>, ()> {
    let mut ea_list = aligned_buffer(5 + EA_NAME.len() + 1);
    let ea_list_bytes = ea_list.as_mut_bytes();
    ea_list_bytes[4] = EA_NAME.len() as u8;
    ea_list_bytes[5..5 + EA_NAME.len()].copy_from_slice(EA_NAME);
    let mut buffer = aligned_buffer(512);
    let mut io_status = IO_STATUS_BLOCK::default();
    let status = unsafe {
        NtQueryEaFile(
            handle,
            &mut io_status,
            buffer.storage.as_mut_ptr() as _,
            buffer.length as u32,
            true,
            ea_list.storage.as_ptr() as _,
            ea_list.length as u32,
            std::ptr::null(),
            true,
        )
    };

    if status == STATUS_NO_EAS_ON_FILE || status == STATUS_NONEXISTENT_EA_ENTRY {
        return Ok(None);
    }

    if status < 0 || io_status.Information > buffer.length {
        return Err(());
    }

    let length = io_status.Information;
    let offset = offset_of!(FILE_FULL_EA_INFORMATION, EaName);
    let buffer = buffer.as_bytes();

    if length < offset + EA_NAME.len() + 1 || buffer[4] != 0 || buffer[5] as usize != EA_NAME.len()
    {
        return Err(());
    }

    let value_length = u16::from_le_bytes([buffer[6], buffer[7]]) as usize;
    let value_start = offset + EA_NAME.len() + 1;
    let value_end = value_start.checked_add(value_length).ok_or(())?;
    let next_entry_offset = u32::from_le_bytes(buffer[0..4].try_into().map_err(|_| ())?);

    if next_entry_offset != 0
        || &buffer[offset..offset + EA_NAME.len()] != EA_NAME
        || buffer[offset + EA_NAME.len()] != 0
        || value_end > length
        || length - value_end >= size_of::<u32>()
        || buffer[value_end..length].iter().any(|byte| *byte != 0)
    {
        return Err(());
    }

    if value_length == 0 {
        return Ok(None);
    }

    decode(&buffer[value_start..value_end], key).map(Some)
}

pub(super) fn write(handle: HANDLE, marker: &Marker, key: &[u8; 32]) -> Result<(), ()> {
    let value = encode(marker, key);
    set(handle, &value)
}

pub(super) fn clear(handle: HANDLE) -> Result<(), ()> {
    set(handle, &[])
}

pub(super) fn creation_ea(marker: &Marker, key: &[u8; 32]) -> EaBuffer {
    let value = encode(marker, key);
    ea_record(&value)
}

fn set(handle: HANDLE, value: &[u8]) -> Result<(), ()> {
    let buffer = ea_record(value);
    let mut io_status = IO_STATUS_BLOCK::default();
    let status = unsafe {
        NtSetEaFile(
            handle,
            &mut io_status,
            buffer.storage.as_ptr() as _,
            buffer.length as u32,
        )
    };

    if status >= 0 { Ok(()) } else { Err(()) }
}

fn ea_record(value: &[u8]) -> EaBuffer {
    let offset = offset_of!(FILE_FULL_EA_INFORMATION, EaName);
    let length = offset + EA_NAME.len() + 1 + value.len();
    let mut buffer = aligned_buffer(length);
    let bytes = buffer.as_mut_bytes();

    bytes[5] = EA_NAME.len() as u8;
    bytes[6..8].copy_from_slice(&(value.len() as u16).to_le_bytes());
    bytes[offset..offset + EA_NAME.len()].copy_from_slice(EA_NAME);
    bytes[offset + EA_NAME.len()] = 0;
    bytes[offset + EA_NAME.len() + 1..length].copy_from_slice(value);

    buffer
}

fn aligned_buffer(length: usize) -> EaBuffer {
    EaBuffer {
        storage: vec![0; length.div_ceil(size_of::<u32>())],
        length,
    }
}

fn encode(marker: &Marker, key: &[u8; 32]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(PAYLOAD_LENGTH + MAC_LENGTH);

    payload.push(FORMAT);
    payload.push(marker.role as u8);
    payload.extend_from_slice(&marker.namespace);
    payload.extend_from_slice(&marker.expected);
    payload.extend_from_slice(&marker.replacement);
    payload.extend_from_slice(&marker.self_volume.to_le_bytes());
    payload.extend_from_slice(&marker.self_id);
    payload.extend_from_slice(&marker.peer_volume.to_le_bytes());
    payload.extend_from_slice(&marker.peer_id);

    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts fixed-size keys");
    mac.update(&payload);
    payload.extend_from_slice(&mac.finalize().into_bytes());

    payload
}

fn decode(value: &[u8], key: &[u8; 32]) -> Result<Marker, ()> {
    if value.len() != PAYLOAD_LENGTH + MAC_LENGTH || value[0] != FORMAT {
        return Err(());
    }

    let (payload, supplied_mac) = value.split_at(PAYLOAD_LENGTH);
    let mut mac = HmacSha256::new_from_slice(key).map_err(|_| ())?;
    mac.update(payload);
    mac.verify_slice(supplied_mac).map_err(|_| ())?;

    let role = ReceiptRole::from_byte(payload[1]).ok_or(())?;
    let mut offset = 2;
    let namespace = take_32(payload, &mut offset)?;
    let expected = take_32(payload, &mut offset)?;
    let replacement = take_32(payload, &mut offset)?;
    let self_volume = take_u64(payload, &mut offset)?;
    let self_id = take_16(payload, &mut offset)?;
    let peer_volume = take_u64(payload, &mut offset)?;
    let peer_id = take_16(payload, &mut offset)?;

    Ok(Marker {
        role,
        namespace,
        expected,
        replacement,
        self_volume,
        self_id,
        peer_volume,
        peer_id,
    })
}

fn take_32(source: &[u8], offset: &mut usize) -> Result<[u8; 32], ()> {
    let value = source
        .get(*offset..*offset + 32)
        .ok_or(())?
        .try_into()
        .map_err(|_| ())?;
    *offset += 32;
    Ok(value)
}

fn take_16(source: &[u8], offset: &mut usize) -> Result<[u8; 16], ()> {
    let value = source
        .get(*offset..*offset + 16)
        .ok_or(())?
        .try_into()
        .map_err(|_| ())?;
    *offset += 16;
    Ok(value)
}

fn take_u64(source: &[u8], offset: &mut usize) -> Result<u64, ()> {
    let value = source.get(*offset..*offset + 8).ok_or(())?;
    *offset += 8;
    Ok(u64::from_le_bytes(value.try_into().map_err(|_| ())?))
}
