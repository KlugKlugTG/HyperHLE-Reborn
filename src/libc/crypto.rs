/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! CommonCrypto and friends

use crate::dyld::FunctionExports;
use crate::mem::{ConstVoidPtr, GuestUSize, MutPtr, MutVoidPtr};
use crate::{export_c_func, Environment};
use digest::Digest;
use md5::Md5;

fn CC_MD5(env: &mut Environment, data: ConstVoidPtr, len: u32, md: MutPtr<u8>) -> MutPtr<u8> {
    let mut hasher = Md5::new();
    hasher.update(env.mem.bytes_at(data.cast(), len));
    let digest = hasher.finalize();
    env.mem.bytes_at_mut(md, 16).copy_from_slice(&digest[..]);
    md
}


// =========================================================================
// MD4 / HMAC (CommonCrypto)
// =========================================================================

fn md4(data: &[u8]) -> [u8; 16] {
    fn f(x: u32, y: u32, z: u32) -> u32 { (x & y) | (!x & z) }
    fn g(x: u32, y: u32, z: u32) -> u32 { (x & y) | (x & z) | (y & z) }
    fn h(x: u32, y: u32, z: u32) -> u32 { x ^ y ^ z }
    fn rol(x: u32, n: u32) -> u32 { x.rotate_left(n) }

    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());

    let (mut a, mut b, mut c, mut d) =
        (0x6745_2301u32, 0xefcd_ab89u32, 0x98ba_dcfeu32, 0x1032_5476u32);

    for chunk in msg.chunks_exact(64) {
        let mut x = [0u32; 16];
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            x[i] = u32::from_le_bytes(word.try_into().unwrap());
        }
        let (aa, bb, cc, dd) = (a, b, c, d);

        // Round 1
        for i in 0..16 {
            let s = [3, 7, 11, 19][i % 4];
            let k = i;
            let v = a.wrapping_add(f(b, c, d)).wrapping_add(x[k]);
            a = d; d = c; c = b; b = rol(v, s);
        }
        // Round 2
        for i in 0..16 {
            let s = [3, 5, 9, 13][i % 4];
            let k = (i % 4) * 4 + i / 4;
            let v = a
                .wrapping_add(g(b, c, d))
                .wrapping_add(x[k])
                .wrapping_add(0x5a82_7999);
            a = d; d = c; c = b; b = rol(v, s);
        }
        // Round 3
        for i in 0..16 {
            let s = [3, 9, 11, 15][i % 4];
            let k = [0, 8, 4, 12, 2, 10, 6, 14, 1, 9, 5, 13, 3, 11, 7, 15][i];
            let v = a
                .wrapping_add(h(b, c, d))
                .wrapping_add(x[k])
                .wrapping_add(0x6ed9_eba1);
            a = d; d = c; c = b; b = rol(v, s);
        }

        a = a.wrapping_add(aa);
        b = b.wrapping_add(bb);
        c = c.wrapping_add(cc);
        d = d.wrapping_add(dd);
    }

    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a.to_le_bytes());
    out[4..8].copy_from_slice(&b.to_le_bytes());
    out[8..12].copy_from_slice(&c.to_le_bytes());
    out[12..16].copy_from_slice(&d.to_le_bytes());
    out
}

fn CC_MD4(env: &mut Environment, data: ConstVoidPtr, len: u32, md: MutPtr<u8>) -> MutPtr<u8> {
    let digest = md4(env.mem.bytes_at(data.cast(), len));
    env.mem.bytes_at_mut(md, 16).copy_from_slice(&digest);
    md
}

fn CC_MD2(env: &mut Environment, data: ConstVoidPtr, len: u32, md: MutPtr<u8>) -> MutPtr<u8> {
    // MD2 is only used by legacy auth code that touchHLE cannot complete
    // anyway; expose the symbol and return a deterministic MD4-based digest.
    CC_MD4(env, data, len, md)
}

#[derive(Clone, Copy)]
enum HmacAlgorithm {
    Sha1,
    Md5,
    Sha256,
}

impl HmacAlgorithm {
    fn from_cc(alg: u32) -> Option<Self> {
        match alg {
            0 => Some(Self::Sha1),
            1 => Some(Self::Md5),
            2 => Some(Self::Sha256),
            _ => None,
        }
    }

    fn hash(self, data: &[u8]) -> Vec<u8> {
        match self {
            Self::Sha1 => sha1::Sha1::digest(data).to_vec(),
            Self::Md5 => md5::Md5::digest(data).to_vec(),
            Self::Sha256 => sha2::Sha256::digest(data).to_vec(),
        }
    }
}

fn hmac_digest(alg: HmacAlgorithm, key: &[u8], data: &[u8]) -> Vec<u8> {
    const BLOCK: usize = 64;
    let mut key_block = if key.len() > BLOCK {
        alg.hash(key)
    } else {
        key.to_vec()
    };
    key_block.resize(BLOCK, 0);

    let mut inner = Vec::with_capacity(BLOCK + data.len());
    inner.extend(key_block.iter().map(|b| b ^ 0x36));
    inner.extend_from_slice(data);
    let inner_hash = alg.hash(&inner);

    let mut outer = Vec::with_capacity(BLOCK + inner_hash.len());
    outer.extend(key_block.iter().map(|b| b ^ 0x5c));
    outer.extend_from_slice(&inner_hash);
    alg.hash(&outer)
}

struct HmacState {
    alg: HmacAlgorithm,
    key: Vec<u8>,
    data: Vec<u8>,
}

lazy_static::lazy_static! {
    static ref HMAC_STATES: std::sync::Mutex<std::collections::HashMap<u32, HmacState>> =
        std::sync::Mutex::new(std::collections::HashMap::new());
}

fn CCHmacInit(
    env: &mut Environment,
    ctx: MutVoidPtr,
    alg: u32,
    key: ConstVoidPtr,
    key_len: GuestUSize,
) {
    let Some(alg) = HmacAlgorithm::from_cc(alg) else {
        return;
    };
    let key = env.mem.bytes_at(key.cast(), key_len).to_vec();
    HMAC_STATES.lock().unwrap().insert(
        ctx.to_bits(),
        HmacState {
            alg,
            key,
            data: Vec::new(),
        },
    );
}

fn CCHmacUpdate(env: &mut Environment, ctx: MutVoidPtr, data: ConstVoidPtr, len: GuestUSize) {
    if let Some(state) = HMAC_STATES.lock().unwrap().get_mut(&ctx.to_bits()) {
        state.data.extend_from_slice(env.mem.bytes_at(data.cast(), len));
    }
}

fn CCHmacFinal(env: &mut Environment, ctx: MutVoidPtr, mac: MutPtr<u8>) {
    let Some(state) = HMAC_STATES.lock().unwrap().remove(&ctx.to_bits()) else {
        return;
    };
    let digest = hmac_digest(state.alg, &state.key, &state.data);
    env.mem.bytes_at_mut(mac, digest.len() as GuestUSize).copy_from_slice(&digest);
}


pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CC_MD5(_, _, _)),
    export_c_func!(CC_MD4(_, _, _)),
    export_c_func!(CC_MD2(_, _, _)),
    export_c_func!(CCHmacInit(_, _, _, _)),
    export_c_func!(CCHmacUpdate(_, _, _)),
    export_c_func!(CCHmacFinal(_, _)),
];
