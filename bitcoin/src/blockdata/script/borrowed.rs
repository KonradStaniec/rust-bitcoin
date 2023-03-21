// Written in 2014 by Andrew Poelstra <apoelstra@wpsoftware.net>
// SPDX-License-Identifier: CC0-1.0

use core::convert::{TryFrom, TryInto};
use core::fmt;
#[cfg(rust_v_1_53)]
use core::ops::Bound;
use core::ops::{Index, Range, RangeFull, RangeFrom, RangeTo, RangeInclusive, RangeToInclusive};
use crate::blockdata::opcodes::{self, all::*};
use crate::blockdata::script::{bytes_to_asm_fmt, Builder, Instruction, Instructions, InstructionIndices, ScriptBuf};
#[cfg(feature = "bitcoinconsensus")]
use crate::blockdata::script::Error;
use crate::consensus::Encodable;
use crate::hash_types::{ScriptHash, WScriptHash};
use crate::hashes::Hash;

use crate::policy::DUST_RELAY_TX_FEE;
use crate::prelude::*;


/// Bitcoin script slice.
///
/// *[See also the `bitcoin::blockdata::script` module](crate::blockdata::script).*
///
/// `Script` is a script slice, the most primitive script type. It's usually seen in its borrowed
/// form `&Script`. It is always encoded as a series of bytes representing the opcodes and data
/// pushes.
///
/// ## Validity
///
/// `Script` does not have any validity invariants - it's essentially just a marked slice of
/// bytes. This is similar to [`Path`](std::path::Path) vs [`OsStr`](std::ffi::OsStr) where they
/// are trivially cast-able to each-other and `Path` doesn't guarantee being a usable FS path but
/// having a newtype still has value because of added methods, readability and basic type checking.
///
/// Although at least data pushes could be checked not to overflow the script, bad scripts are
/// allowed to be in a transaction (outputs just become unspendable) and there even are such
/// transactions in the chain. Thus we must allow such scripts to be placed in the transaction.
///
/// ## Slicing safety
///
/// Slicing is similar to how `str` works: some ranges may be incorrect and indexing by
/// `usize` is not supported. However, as opposed to `std`, we have no way of checking
/// correctness without causing linear complexity so there are **no panics on invalid
/// ranges!** If you supply an invalid range, you'll get a garbled script.
///
/// The range is considered valid if it's at a boundary of instruction. Care must be taken
/// especially with push operations because you could get a reference to arbitrary
/// attacker-supplied bytes that look like a valid script.
///
/// It is recommended to use `.instructions()` method to get an iterator over script
/// instructions and work with that instead.
///
/// ## Memory safety
///
/// The type is `#[repr(transparent)]` for internal purposes only!
/// No consumer crate may rely on the represenation of the struct!
///
/// ## References
///
///
/// ### Bitcoin Core References
///
/// * [CScript definition](https://github.com/bitcoin/bitcoin/blob/d492dc1cdaabdc52b0766bf4cba4bd73178325d0/src/script/script.h#L410)
///
#[derive(PartialOrd, Ord, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Script(pub (in crate::blockdata::script) [u8]);

impl ToOwned for Script {
    type Owned = ScriptBuf;

    fn to_owned(&self) -> Self::Owned {
        ScriptBuf(self.0.to_owned())
    }
}

impl Script {
    /// Treat byte slice as `Script`
    #[inline]
    pub fn from_bytes(bytes: &[u8]) -> &Script {
        // SAFETY: copied from `std`
        // The pointer was just created from a reference which is still alive.
        // Casting slice pointer to a transparent struct wrapping that slice is sound (same
        // layout).
        unsafe {
            &*(bytes as *const [u8] as *const Script)
        }
    }

    /// Treat mutable byte slice as `Script`
    #[inline]
    pub fn from_bytes_mut(bytes: &mut [u8]) -> &mut Script {
        // SAFETY: copied from `std`
        // The pointer was just created from a reference which is still alive.
        // Casting slice pointer to a transparent struct wrapping that slice is sound (same
        // layout).
        // Function signature prevents callers from accessing `bytes` while the returned reference
        // is alive.
        unsafe {
            &mut *(bytes as *mut [u8] as *mut Script)
        }
    }

    /// Returns the script data as a byte slice.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Returns the script data as a mutable byte slice.
    #[inline]
    pub fn as_mut_bytes(&mut self) -> &mut [u8] {
        &mut self.0
    }

    /// Creates a new empty script.
    #[inline]
    pub fn empty() -> &'static Script { Script::from_bytes(&[]) }

    /// Creates a new script builder
    pub fn builder() -> Builder {
      Builder::new()
    }

    /// Returns 160-bit hash of the script.
    #[inline]
    pub fn script_hash(&self) -> ScriptHash {
        ScriptHash::hash(self.as_bytes())
    }

    /// Returns 256-bit hash of the script for P2WSH outputs.
    #[inline]
    pub fn wscript_hash(&self) -> WScriptHash {
        WScriptHash::hash(self.as_bytes())
    }

    /// Returns the length in bytes of the script.
    #[inline]
    pub fn len(&self) -> usize { self.0.len() }

    /// Returns whether the script is the empty script.
    #[inline]
    pub fn is_empty(&self) -> bool { self.0.is_empty() }

    /// Returns a copy of the script data.
    #[inline]
    pub fn to_bytes(&self) -> Vec<u8> { self.0.to_owned() }

    /// Returns an iterator over script bytes.
    #[inline]
    pub fn bytes(&self) -> Bytes<'_> {
        Bytes(self.as_bytes().iter().copied())
    }

    /// Checks whether a script pubkey is a P2SH output.
    #[inline]
    pub fn is_p2sh(&self) -> bool {
        self.0.len() == 23
            && self.0[0] == OP_HASH160.to_u8()
            && self.0[1] == OP_PUSHBYTES_20.to_u8()
            && self.0[22] == OP_EQUAL.to_u8()
    }

    /// Checks whether a script pubkey is a P2PKH output.
    #[inline]
    pub fn is_p2pkh(&self) -> bool {
        self.0.len() == 25
            && self.0[0] == OP_DUP.to_u8()
            && self.0[1] == OP_HASH160.to_u8()
            && self.0[2] == OP_PUSHBYTES_20.to_u8()
            && self.0[23] == OP_EQUALVERIFY.to_u8()
            && self.0[24] == OP_CHECKSIG.to_u8()
    }

    /// Checks whether a script pubkey is a P2PK output.
    ///
    /// You can obtain the public key, if its valid,
    /// by calling [`p2pk_public_key()`](Self::p2pk_public_key)
    #[inline]
    pub fn is_p2pk(&self) -> bool {
        self.p2pk_pubkey_bytes().is_some()
    }

    /// Returns the bytes of the (possibly invalid) public key if this script is P2PK.
    #[inline]
    pub(in crate::blockdata::script) fn p2pk_pubkey_bytes(&self) -> Option<&[u8]> {
        match self.len() {
            67 if self.0[0] == OP_PUSHBYTES_65.to_u8()
                    && self.0[66] == OP_CHECKSIG.to_u8() =>  {
                Some(&self.0[1..66])
            }
            35 if self.0[0] == OP_PUSHBYTES_33.to_u8()
                    && self.0[34] == OP_CHECKSIG.to_u8() =>  {
                Some(&self.0[1..34])
            }
            _ => None
        }
    }

    /// Check if this is an OP_RETURN output.
    #[inline]
    pub fn is_op_return (&self) -> bool {
        match self.0.first() {
            Some(b) => *b == OP_RETURN.to_u8(),
            None => false
        }
    }

    /// Checks whether a script can be proven to have no satisfying input.
    #[inline]
    pub fn is_provably_unspendable(&self) -> bool {
        use crate::blockdata::opcodes::Class::{ReturnOp, IllegalOp};

        match self.0.first() {
            Some(b) => {
                let first = opcodes::All::from(*b);
                let class = first.classify(opcodes::ClassifyContext::Legacy);

                class == ReturnOp || class == IllegalOp
            },
            None => false,
        }
    }

    /// Iterates over the script instructions.
    ///
    /// Each returned item is a nested enum covering opcodes, datapushes and errors.
    /// At most one error will be returned and then the iterator will end. To instead iterate over
    /// the script as sequence of bytes call the [`bytes`](Self::bytes) method.
    ///
    /// To force minimal pushes, use [`instructions_minimal`](Self::instructions_minimal).
    #[inline]
    pub fn instructions(&self) -> Instructions {
        Instructions {
            data: self.0.iter(),
            enforce_minimal: false,
        }
    }

    /// Iterates over the script instructions while enforcing minimal pushes.
    ///
    /// This is similar to [`instructions`](Self::instructions) but an error is returned if a push
    /// is not minimal.
    #[inline]
    pub fn instructions_minimal(&self) -> Instructions {
        Instructions {
            data: self.0.iter(),
            enforce_minimal: true,
        }
    }

    /// Iterates over the script instructions and their indices.
    ///
    /// Unless the script contains an error, the returned item consists of an index pointing to the
    /// position in the script where the instruction begins and the decoded instruction - either an
    /// opcode or data push.
    ///
    /// To force minimal pushes, use [`Self::instruction_indices_minimal`].
    #[inline]
    pub fn instruction_indices(&self) -> InstructionIndices {
        InstructionIndices::from_instructions(self.instructions())
    }

    /// Iterates over the script instructions and their indices while enforcing minimal pushes.
    ///
    /// This is similar to [`instruction_indices`](Self::instruction_indices) but an error is
    /// returned if a push is not minimal.
    #[inline]
    pub fn instruction_indices_minimal(&self) -> InstructionIndices {
        InstructionIndices::from_instructions(self.instructions_minimal())
    }

    /// Shorthand for [`Self::verify_with_flags`] with flag [bitcoinconsensus::VERIFY_ALL].
    ///
    /// # Parameters
    ///  * `index` - The input index in spending which is spending this transaction.
    ///  * `amount` - The amount this script guards.
    ///  * `spending_tx` - The transaction that attempts to spend the output holding this script.
    #[cfg(feature="bitcoinconsensus")]
    #[cfg_attr(docsrs, doc(cfg(feature = "bitcoinconsensus")))]
    pub fn verify (&self, index: usize, amount: crate::Amount, spending_tx: &[u8]) -> Result<(), Error> {
        self.verify_with_flags(index, amount, spending_tx, bitcoinconsensus::VERIFY_ALL)
    }

    /// Verifies spend of an input script.
    ///
    /// # Parameters
    ///  * `index` - The input index in spending which is spending this transaction.
    ///  * `amount` - The amount this script guards.
    ///  * `spending_tx` - The transaction that attempts to spend the output holding this script.
    ///  * `flags` - Verification flags, see [`bitcoinconsensus::VERIFY_ALL`] and similar.
    #[cfg(feature="bitcoinconsensus")]
    #[cfg_attr(docsrs, doc(cfg(feature = "bitcoinconsensus")))]
    pub fn verify_with_flags<F: Into<u32>>(&self, index: usize, amount: crate::Amount, spending_tx: &[u8], flags: F) -> Result<(), Error> {
        Ok(bitcoinconsensus::verify_with_flags (&self.0[..], amount.to_sat(), spending_tx, index, flags.into())?)
    }

    /// Writes the assembly decoding of the script to the formatter.
    pub fn fmt_asm(&self, f: &mut dyn fmt::Write) -> fmt::Result {
        bytes_to_asm_fmt(self.as_ref(), f)
    }

    /// Returns the assembly decoding of the script.
    pub fn to_asm_string(&self) -> String {
        let mut buf = String::new();
        self.fmt_asm(&mut buf).unwrap();
        buf
    }

    /// Formats the script as lower-case hex.
    ///
    /// This is a more convenient and performant way to write `format!("{:x}", script)`.
    /// For better performance you should generally prefer displaying the script but if `String` is
    /// required (this is common in tests) this method is can be used.
    pub fn to_hex_string(&self) -> String {
        self.as_bytes().to_lower_hex_string()
    }

    /// Returns the first opcode of the script (if there is any).
    pub fn first_opcode(&self) -> Option<opcodes::All> {
        self.as_bytes().first().copied().map(From::from)
    }

    /// Iterates the script to find the last opcode.
    ///
    /// Returns `None` is the instruction is data push or if the script is empty.
    pub(in crate::blockdata::script) fn last_opcode(&self) -> Option<opcodes::All> {
        match self.instructions().last() {
            Some(Ok(Instruction::Op(op))) => Some(op),
            _ => None,
        }
    }

    /// Converts a [`Box<Script>`](Box) into a [`ScriptBuf`] without copying or allocating.
    #[must_use = "`self` will be dropped if the result is not used"]
    pub fn into_script_buf(self: Box<Self>) -> ScriptBuf {
        let rw = Box::into_raw(self) as *mut [u8];
        // SAFETY: copied from `std`
        // The pointer was just created from a box without deallocating
        // Casting a transparent struct wrapping a slice to the slice pointer is sound (same
        // layout).
        let inner = unsafe { Box::from_raw(rw) };
        ScriptBuf(Vec::from(inner))
    }
}

/// Iterator over bytes of a script
pub struct Bytes<'a>(core::iter::Copied<core::slice::Iter<'a, u8>>);

impl Iterator for Bytes<'_> {
    type Item = u8;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }

    #[inline]
    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        self.0.nth(n)
    }
}

impl DoubleEndedIterator for Bytes<'_> {
    #[inline]
    fn next_back(&mut self) -> Option<Self::Item> {
        self.0.next_back()
    }

    #[inline]
    fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
        self.0.nth_back(n)
    }
}

impl ExactSizeIterator for Bytes<'_> {}
impl core::iter::FusedIterator for Bytes<'_> {}

macro_rules! delegate_index {
    ($($type:ty),* $(,)?) => {
        $(
            /// Script subslicing operation - read [slicing safety](#slicing-safety)!
            impl Index<$type> for Script {
                type Output = Self;

                #[inline]
                fn index(&self, index: $type) -> &Self::Output {
                    Self::from_bytes(&self.0[index])
                }
            }
        )*
    }
}

delegate_index!(Range<usize>, RangeFrom<usize>, RangeTo<usize>, RangeFull, RangeInclusive<usize>, RangeToInclusive<usize>);
#[cfg(rust_v_1_53)]
#[cfg_attr(docsrs, doc(cfg(rust_v_1_53)))]
delegate_index!((Bound<usize>, Bound<usize>));
