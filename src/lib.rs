use std::alloc::{alloc, Layout};

use blake_hash::Blake256;
use digest::{Digest, FixedOutput, Input, Reset};
use digest::generic_array::GenericArray;
use digest::generic_array::typenum::{U200, U32};
use groestl::Groestl256;
use jh_x86_64::Jh256;
use skein_hash::Skein256;

use crate::aes::aes_round;
use crate::aes::derive_key;
use crate::aes::xor;
use crate::u64p::U64p;
use slice_cast::cast_mut;

mod aes;
mod u64p;

const SCRATCH_PAD_SIZE: usize = 1 << 21;
const ROUNDS: usize = 524_288;

#[repr(align(16))]
/// Helper to enforce 16 byte alignment
struct A16<T>(pub T);

#[derive(Default, Clone)]
pub struct CryptoNight {
    internal_hasher: sha3::Keccak256Full,
}


impl CryptoNight {
    fn init_scratchpad(keccac: &[u8], scratchpad: &mut [u8]) {
        let round_keys_buffer = derive_key(&keccac[..32]);

        let mut blocks = [0u8; 128];
        blocks.copy_from_slice(&keccac[64..192]);

        for scratchpad_chunk in scratchpad.chunks_exact_mut(blocks.len()) {
            for block in blocks.chunks_exact_mut(16) {
                for key in round_keys_buffer.chunks_exact(16) {
                    aes_round(block, key);
                }
            }

            scratchpad_chunk.copy_from_slice(&blocks);
        }
    }

    fn main_loop(mut a: U64p, mut b: U64p, scratch_pad: &mut [u8]) {
        // Cast to u128 for easier handling. Scratch pad is only used in 16 byte blocks
        let scratch_pad: &mut [U64p] = unsafe { cast_mut(scratch_pad)};

        for _ in 0..ROUNDS {
            // First transfer
            let address: usize = a.into();
            aes_round(&mut scratch_pad[address].as_mut(), a.as_ref());
            let tmp = b;
            b = scratch_pad[address];
            scratch_pad[address] = scratch_pad[address] ^ tmp;

            // Second transfer
            let address: usize = b.into();
            let tmp = a + b * scratch_pad[address];
            a = scratch_pad[address] ^ tmp;
            scratch_pad[address] = tmp;
        }
    }

    fn finalize_state(keccac: &mut [u8], scratch_pad: &[u8]) {
        let round_keys_buffer = derive_key(&keccac[32..64]);
        let final_block = &mut keccac[64..192];
        for scratchpad_chunk in scratch_pad.chunks_exact(128) {
            xor(final_block, scratchpad_chunk);
            for block in final_block.chunks_exact_mut(16) {
                for key in round_keys_buffer.chunks_exact(16) {
                    aes_round(block, key);
                }
            }
        }
    }

    fn hash_final_state(state: &[u8]) -> GenericArray<u8, <Self as FixedOutput>::OutputSize> {
        match state[0] & 3 {
            0 => Blake256::digest(&state),
            1 => Groestl256::digest(&state),
            2 => Jh256::digest(&state),
            3 => Skein256::digest(&state),
            _ => unreachable!("Invalid output option")
        }
    }
}

impl Input for CryptoNight {
    fn input<B: AsRef<[u8]>>(&mut self, data: B) {
        Input::input(&mut self.internal_hasher, data);
    }
}

impl Reset for CryptoNight {
    fn reset(&mut self) {
        Reset::reset(&mut self.internal_hasher);
    }
}

impl FixedOutput for CryptoNight {
    type OutputSize = U32;

    fn fixed_result(self) -> GenericArray<u8, Self::OutputSize> {
        let mut keccac = A16(self.internal_hasher.fixed_result());
        let keccac = &mut keccac.0;

        let mut scratch_pad = unsafe {
            let buffer = alloc(Layout::from_size_align_unchecked(SCRATCH_PAD_SIZE, 16));
            Vec::from_raw_parts(buffer, SCRATCH_PAD_SIZE, SCRATCH_PAD_SIZE)
        };

        Self::init_scratchpad(keccac, &mut scratch_pad);

        let a = U64p::from(&keccac[..16]) ^ U64p::from(&keccac[32..48]);
        let b = U64p::from(&keccac[16..32]) ^ U64p::from(&keccac[48..64]);

        CryptoNight::main_loop(a, b, &mut scratch_pad);

        CryptoNight::finalize_state(keccac, &scratch_pad);

        tiny_keccak::keccakf(unsafe { &mut *(keccac as *mut GenericArray<u8, U200> as *mut [u64; 25]) });

        Self::hash_final_state(&keccac)
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_samples() {
        // Samples taken from
        validate_sample(
            b"",
            b"eb14e8a833fac6fe9a43b57b336789c46ffe93f2868452240720607b14387e11",
        );
        validate_sample(
            b"This is a test",
            b"a084f01d1437a09c6985401b60d43554ae105802c5f5d8a9b3253649c0be6605",
        );
    }

    fn validate_sample(input: &[u8], hash: &[u8]) {
        let hash = hex::decode(hash).unwrap();

        let actual_result = CryptoNight::digest(input);

        assert_eq!(actual_result.as_slice(), hash.as_slice())
    }
}