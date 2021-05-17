// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

// TODO - Optimize the XORs
// TODO - Maybe use macros to specialize BlockEngine for encryption or decryption?
// TODO - I think padding could be done better. Maybe macros for BlockEngine would help this too.

use std::cmp;
use std::iter::repeat;

use buffer::{ReadBuffer, WriteBuffer, OwnedReadBuffer, OwnedWriteBuffer, BufferResult,
    RefReadBuffer, RefWriteBuffer};
use buffer::BufferResult::{BufferUnderflow, BufferOverflow};
use cryptoutil::{self, symm_enc_or_dec};
use symmetriccipher::{BlockEncryptor, BlockEncryptorX8, Encryptor, BlockDecryptor, Decryptor,
    SynchronousStreamCipher, SymmetricCipherError};
use symmetriccipher::SymmetricCipherError::{InvalidPadding, InvalidLength};

/// The BlockProcessor trait is used to implement modes that require processing complete blocks of
/// data. The methods of this trait are called by the BlockEngine which is in charge of properly
/// buffering input data.
trait BlockProcessor {
    /// Process a block of data. The in_hist and out_hist parameters represent the input and output
    /// when the last block was processed. These values are necessary for certain modes.
    fn process_block(&mut self, in_hist: &[u8], out_hist: &[u8], input: &[u8], output: &mut [u8]);
}

/// A PaddingProcessor handles adding or removing padding
pub trait PaddingProcessor {
    /// Add padding to the last block of input data
    /// If the mode can't handle a non-full block, it signals that error by simply leaving the block
    /// as it is which will be detected as an InvalidLength error.
    fn pad_input<W: WriteBuffer>(&mut self, input_buffer: &mut W);

    /// Remove padding from the last block of output data
    /// If false is returned, the processing fails
    fn strip_output<R: ReadBuffer>(&mut self, output_buffer: &mut R) -> bool;
}

/// The BlockEngine is implemented as a state machine with the following states. See comments in the
/// BlockEngine code for more information on the states.
#[derive(Clone, Copy)]
enum BlockEngineState {
    FastMode,
    NeedInput,
    NeedOutput,
    LastInput,
    LastInput2,
    Finished,
    Error(SymmetricCipherError)
}

/// BlockEngine buffers input and output data and handles sending complete block of data to the
/// Processor object. Additionally, BlockEngine handles logic necessary to add or remove padding by
/// calling the appropriate methods on the Processor object.
struct BlockEngine<P, X> {
    /// The block sized expected by the Processor
    block_size: usize,

    /// in_hist and out_hist keep track of data that was input to and output from the last
    /// invocation of the process_block() method of the Processor. Depending on the mode, these may
    /// be empty vectors if history is not needed.
    in_hist: Vec<u8>,
    out_hist: Vec<u8>,

    /// If some input data is supplied, but not a complete blocks worth, it is stored in this buffer
    /// until enough arrives that it can be passed to the process_block() method of the Processor.
    in_scratch: OwnedWriteBuffer,

    /// If input data is processed but there isn't enough space in the output buffer to store it,
    /// it is written into out_write_scratch. OwnedWriteBuffer's may be converted into
    /// OwnedReaderBuffers without re-allocating, so, after being written, out_write_scratch is
    /// turned into out_read_scratch. After that, if is written to the output as more output becomes
    /// available. The main point is - only out_write_scratch or out_read_scratch contains a value
    /// at any given time; never both.
    out_write_scratch: Option<OwnedWriteBuffer>,
    out_read_scratch: Option<OwnedReadBuffer>,

    /// The processor that implements the particular block mode.
    processor: P,

    /// The padding processor
    padding: X,

    /// The current state of the operation.
    state: BlockEngineState
}

fn update_history(in_hist: &mut [u8], out_hist: &mut [u8], last_in: &[u8], last_out: &[u8]) {
    let in_hist_len = in_hist.len();
    if in_hist_len > 0 {
        cryptoutil::copy_memory(
            &last_in[last_in.len() - in_hist_len..],
            in_hist);
    }
    let out_hist_len = out_hist.len();
    if out_hist_len > 0 {
        cryptoutil::copy_memory(
            &last_out[last_out.len() - out_hist_len..],
            out_hist);
    }
}

impl <P: BlockProcessor, X: PaddingProcessor> BlockEngine<P, X> {
    /// Create a new BlockProcessor instance with the given processor and block_size. No history
    /// will be saved.
    fn new(processor: P, padding: X, block_size: usize) -> BlockEngine<P, X> {
        BlockEngine {
            block_size: block_size,
            in_hist: Vec::new(),
            out_hist: Vec::new(),
            in_scratch: OwnedWriteBuffer::new(repeat(0).take(block_size).collect()),
            out_write_scratch: Some(OwnedWriteBuffer::new(repeat(0).take(block_size).collect())),
            out_read_scratch: None,
            processor: processor,
            padding: padding,
            state: BlockEngineState::FastMode
        }
    }

    /// Create a new BlockProcessor instance with the given processor, block_size, and initial input
    /// and output history.
    fn new_with_history(
            processor: P,
            padding: X,
            block_size: usize,
            in_hist: Vec<u8>,
            out_hist: Vec<u8>) -> BlockEngine<P, X> {
        BlockEngine {
            in_hist: in_hist,
            out_hist: out_hist,
            ..BlockEngine::new(processor, padding, block_size)
        }
    }

    /// This implements the FastMode state. Ideally, the encryption or decryption operation should
    /// do the bulk of its work in FastMode. Significantly, FastMode avoids doing copies as much as
    /// possible. The FastMode state does not handle the final block of data.
    fn fast_mode<R: ReadBuffer, W: WriteBuffer>(
            &mut self,
            input: &mut R,
            output: &mut W) -> BlockEngineState {
        fn has_next<R: ReadBuffer, W: WriteBuffer>(
                input: &mut R,
                output: &mut W,
                block_size: usize) -> bool {
            // Not the greater than - very important since this method must never process the last
            // block.
            let enough_input = input.remaining() > block_size;
            let enough_output = output.remaining() >= block_size;
            enough_input && enough_output
        };
        fn split_at<'a>(vec: &'a [u8], at: usize) -> (&'a [u8], &'a [u8]) {
            (&vec[..at], &vec[at..])
        }

        // First block processing. We have to retrieve the history information from self.in_hist and
        // self.out_hist.
        if !has_next(input, output, self.block_size) {
            if input.is_empty() {
                return BlockEngineState::FastMode;
            } else {
                return BlockEngineState::NeedInput;
            }
        } else {
            let next_in = input.take_next(self.block_size);
            let next_out = output.take_next(self.block_size);
            self.processor.process_block(
                &self.in_hist[..],
                &self.out_hist[..],
                next_in,
                next_out);
        }

        // Process all remaing blocks. We can pull the history out of the buffers without having to
        // do any copies
        let next_in_size = self.in_hist.len() + self.block_size;
        let next_out_size = self.out_hist.len() + self.block_size;
        while has_next(input, output, self.block_size) {
            input.rewind(self.in_hist.len());
            let (in_hist, next_in) = split_at(input.take_next(next_in_size), self.in_hist.len());
            output.rewind(self.out_hist.len());
            let (out_hist, next_out) = output.take_next(next_out_size).split_at_mut(
                self.out_hist.len());
            self.processor.process_block(
                in_hist,
                out_hist,
                next_in,
                next_out);
        }

        // Save the history and then transition to the next state
        {
            input.rewind(self.in_hist.len());
            let last_in = input.take_next(self.in_hist.len());
            output.rewind(self.out_hist.len());
            let last_out = output.take_next(self.out_hist.len());
            update_history(
                &mut self.in_hist,
                &mut self.out_hist,
                last_in,
                last_out);
        }
        if input.is_empty() {
            BlockEngineState::FastMode
        } else {
            BlockEngineState::NeedInput
        }
    }

    /// This method implements the BlockEngine state machine.
    fn process<R: ReadBuffer, W: WriteBuffer>(
            &mut self,
            input: &mut R,
            output: &mut W,
            eof: bool) -> Result<BufferResult, SymmetricCipherError> {
        // Process a block of data from in_scratch and write the result to out_write_scratch.
        // Finally, convert out_write_scratch into out_read_scratch.
        fn process_scratch<P: BlockProcessor, X: PaddingProcessor>(me: &mut BlockEngine<P, X>) {
            let mut rin = me.in_scratch.take_read_buffer();
            let mut wout = me.out_write_scratch.take().unwrap();

            {
                let next_in = rin.take_remaining();
                let next_out = wout.take_remaining();
                me.processor.process_block(
                    &me.in_hist[..],
                    &me.out_hist[..],
                    next_in,
                    next_out);
                update_history(
                    &mut me.in_hist,
                    &mut me.out_hist,
                    next_in,
                    next_out);
            }

            let rb = wout.into_read_buffer();
            me.out_read_scratch = Some(rb);
        };

        loop {
            match self.state {
                // FastMode tries to process as much data as possible while minimizing copies.
                // FastMode doesn't make use of the scratch buffers and only updates the history
                // just before exiting.
                BlockEngineState::FastMode => {
                    self.state = self.fast_mode(input, output);
                    match self.state {
                        BlockEngineState::FastMode => {
                            // If FastMode completes but stays in the FastMode state, it means that
                            // we've run out of input data.
                            return Ok(BufferUnderflow);
                        }
                        _ => {}
                    }
                }

                // The NeedInput mode is entered when there isn't enough data to run in FastMode
                // anymore. Input data is buffered in in_scratch until there is a full block or eof
                // occurs. IF eof doesn't occur, the data is processed and then we go to the
                // NeedOutput state. Otherwise, we go to the LastInput state. This state always
                // writes all available data into in_scratch before transitioning to the next state.
                BlockEngineState::NeedInput => {
                    input.push_to(&mut self.in_scratch);
                    if !input.is_empty() {
                        // !is_empty() guarantees two things - in_scratch is full and its not the
                        // last block. This state must never process the last block.
                        process_scratch(self);
                        self.state = BlockEngineState::NeedOutput;
                    } else {
                        if eof {
                            self.state = BlockEngineState::LastInput;
                        } else {
                            return Ok(BufferUnderflow);
                        }
                    }
                }

                // The NeedOutput state just writes buffered processed data to the output stream
                // until all of it has been written.
                BlockEngineState::NeedOutput => {
                    let mut rout = self.out_read_scratch.take().unwrap();
                    rout.push_to(output);
                    if rout.is_empty() {
                        self.out_write_scratch = Some(rout.into_write_buffer());
                        self.state = BlockEngineState::FastMode;
                    } else {
                        self.out_read_scratch = Some(rout);
                        return Ok(BufferOverflow);
                    }
                }

                // None of the other states are allowed to process the last block of data since
                // last block handling is a little tricky due to modes have special needs regarding
                // padding. When the last block of data is detected, this state is transitioned to
                // for handling.
                BlockEngineState::LastInput => {
                    // We we arrive in this state, we know that all input data that is going to be
                    // supplied has been suplied and that that data has been written to in_scratch
                    // by the NeedInput state. Furthermore, we know that one of three things must be
                    // true about in_scratch:
                    // 1) It is empty. This only occurs if the input is zero length. We can do last
                    //    block processing by executing the pad_input() method of the processor
                    //    which may either pad out to a full block or leave it empty, process the
                    //    data if it was padded out to a full block, and then pass it to
                    //    strip_output().
                    // 2) It is partially filled. This will occur if the input data was not a
                    //    multiple of the block size. Processing proceeds identically to case #1.
                    // 3) It is full. This case occurs when the input data was a multiple of the
                    //    block size. This case is a little trickier, since, depending on the mode,
                    //    we might actually have 2 blocks worth of data to process - the last user
                    //    supplied block (currently in in_scratch) and then another block that could
                    //    be added as padding. Processing proceeds by first processing the data in
                    //    in_scratch and writing it to out_scratch. Then, the now-empty in_scratch
                    //    buffer is passed to pad_input() which may leave it empty or write a block
                    //    of padding to it. If no padding is added, processing proceeds as in cases
                    //    #1 and #2. However, if padding is added, now have data in in_scratch and
                    //    also in out_scratch meaning that we can't immediately process the padding
                    //    data since we have nowhere to put it. So, we transition to the LastInput2
                    //    state which will first write out the last non-padding block, then process
                    //    the padding block (in in_scratch) and write it to the now-empty
                    //    out_scratch.
                    if !self.in_scratch.is_full() {
                        self.padding.pad_input(&mut self.in_scratch);
                        if self.in_scratch.is_full() {
                            process_scratch(self);
                            if self.padding.strip_output(self.out_read_scratch.as_mut().unwrap()) {
                                self.state = BlockEngineState::Finished;
                            } else {
                                self.state = BlockEngineState::Error(InvalidPadding);
                            }
                        } else if self.in_scratch.is_empty() {
                            self.state = BlockEngineState::Finished;
                        } else {
                            self.state = BlockEngineState::Error(InvalidLength);
                        }
                    } else {
                        process_scratch(self);
                        self.padding.pad_input(&mut self.in_scratch);
                        if self.in_scratch.is_full() {
                            self.state = BlockEngineState::LastInput2;
                        } else if self.in_scratch.is_empty() {
                            if self.padding.strip_output(self.out_read_scratch.as_mut().unwrap()) {
                                self.state = BlockEngineState::Finished;
                            } else {
                                self.state = BlockEngineState::Error(InvalidPadding);
                            }
                        } else {
                            self.state = BlockEngineState::Error(InvalidLength);
                        }
                    }
                }

                // See the comments on LastInput for more details. This state handles final blocks
                // of data in the case that the input was a multiple of the block size and the mode
                // decided to add a full extra block of padding.
                BlockEngineState::LastInput2 => {
                    let mut rout = self.out_read_scratch.take().unwrap();
                    rout.push_to(output);
                    if rout.is_empty() {
                        self.out_write_scratch = Some(rout.into_write_buffer());
                        process_scratch(self);
                        if self.padding.strip_output(self.out_read_scratch.as_mut().unwrap()) {
                            self.state = BlockEngineState::Finished;
                        } else {
                            self.state = BlockEngineState::Error(InvalidPadding);
                        }
                    } else {
                        self.out_read_scratch = Some(rout);
                        return Ok(BufferOverflow);
                    }
                }

                // The Finished mode just writes the data in out_scratch to the output until there
                // is no more data left.
                BlockEngineState::Finished => {
                    match self.out_read_scratch {
                        Some(ref mut rout) => {
                            rout.push_to(output);
                            if rout.is_empty() {
                                return Ok(BufferUnderflow);
                            } else {
                                return Ok(BufferOverflow);
                            }
                        }
                        None => { return Ok(BufferUnderflow); }
                    }
                }

                // The Error state is used to store error information.
                BlockEngineState::Error(err) => {
                    return Err(err);
                }
            }
        }
    }
    fn reset(&mut self) {
        self.state = BlockEngineState::FastMode;
        self.in_scratch.reset();
        if self.out_read_scratch.is_some() {
            let ors = self.out_read_scratch.take().unwrap();
            let ows = ors.into_write_buffer();
            self.out_write_scratch = Some(ows);
        } else {
            self.out_write_scratch.as_mut().unwrap().reset();
        }
    }
    fn reset_with_history(&mut self, in_hist: &[u8], out_hist: &[u8]) {
        self.reset();
        cryptoutil::copy_memory(in_hist, &mut self.in_hist);
        cryptoutil::copy_memory(out_hist, &mut self.out_hist);
    }
}

/// No padding mode for ECB and CBC encryption
#[derive(Clone, Copy)]
pub struct NoPadding;

impl PaddingProcessor for NoPadding {
    fn pad_input<W: WriteBuffer>(&mut self, _: &mut W) { }
    fn strip_output<R: ReadBuffer>(&mut self, _: &mut R) -> bool { true }
}

/// PKCS padding mode for ECB and CBC encryption
#[derive(Clone, Copy)]
pub struct PkcsPadding;

// This class implements both encryption padding, where padding is added, and decryption padding,
// where padding is stripped. Since BlockEngine doesn't know if its an Encryption or Decryption
// operation, it will call both methods if given a chance. So, this class can't be passed directly
// to BlockEngine. Instead, it must be wrapped with EncPadding or DecPadding which will ensure that
// only the propper methods are called. The client of the library, however, doesn't have to
// distinguish encryption padding handling from decryption padding handline, which is the whole
// point.
impl PaddingProcessor for PkcsPadding {
    fn pad_input<W: WriteBuffer>(&mut self, input_buffer: &mut W) {
        let rem = input_buffer.remaining();
        assert!(rem != 0 && rem <= 255);
        for v in input_buffer.take_remaining().iter_mut() {
            *v = rem as u8;
        }
    }
    fn strip_output<R: ReadBuffer>(&mut self, output_buffer: &mut R) -> bool {
        let last_byte: u8;
        {
            let data = output_buffer.peek_remaining();
            last_byte = *data.last().unwrap();
            for &x in data.iter().rev().take(last_byte as usize) {
                if x != last_byte {
                    return false;
                }
            }
        }
        output_buffer.truncate(last_byte as usize);
        true
    }
}

/// Wraps a PaddingProcessor so that only pad_input() will actually be called.
pub struct EncPadding<X> {
    padding: X
}

impl <X: PaddingProcessor> EncPadding<X> {
    fn wrap(p: X) -> EncPadding<X> { EncPadding { padding: p } }
}

impl <X: PaddingProcessor> PaddingProcessor for EncPadding<X> {
    fn pad_input<W: WriteBuffer>(&mut self, a: &mut W) { self.padding.pad_input(a); }
    fn strip_output<R: ReadBuffer>(&mut self, _: &mut R) -> bool { true }
}

/// Wraps a PaddingProcessor so that only strip_output() will actually be called.
pub struct DecPadding<X> {
    padding: X
}

impl <X: PaddingProcessor> DecPadding<X> {
    fn wrap(p: X) -> DecPadding<X> { DecPadding { padding: p } }
}

impl <X: PaddingProcessor> PaddingProcessor for DecPadding<X> {
    fn pad_input<W: WriteBuffer>(&mut self, _: &mut W) { }
    fn strip_output<R: ReadBuffer>(&mut self, a: &mut R) -> bool { self.padding.strip_output(a) }
}

struct EcbEncryptorProcessor<T> {
    algo: T
}

impl <T: BlockEncryptor> BlockProcessor for EcbEncryptorProcessor<T> {
    fn process_block(&mut self, _: &[u8], _: &[u8], input: &[u8], output: &mut [u8]) {
        self.algo.encrypt_block(input, output);
    }
}

/// ECB Encryption mode
pub struct EcbEncryptor<T, X> {
    block_engine: BlockEngine<EcbEncryptorProcessor<T>, X>
}

impl <T: BlockEncryptor, X: PaddingProcessor> EcbEncryptor<T, X> {
    /// Create a new ECB encryption mode object
    pub fn new(algo: T, padding: X) -> EcbEncryptor<T, EncPadding<X>> {
        let block_size = algo.block_size();
        let processor = EcbEncryptorProcessor {
            algo: algo
        };
        EcbEncryptor {
            block_engine: BlockEngine::new(processor, EncPadding::wrap(padding), block_size)
        }
    }
    pub fn reset(&mut self) {
        self.block_engine.reset();
    }
}

impl <T: BlockEncryptor, X: PaddingProcessor> Encryptor for EcbEncryptor<T, X> {
    fn encrypt(&mut self, input: &mut RefReadBuffer, output: &mut RefWriteBuffer, eof: bool)
            -> Result<BufferResult, SymmetricCipherError> {
        self.block_engine.process(input, output, eof)
    }
}

struct EcbDecryptorProcessor<T> {
    algo: T
}

impl <T: BlockDecryptor> BlockProcessor for EcbDecryptorProcessor<T> {
    fn process_block(&mut self, _: &[u8], _: &[u8], input: &[u8], output: &mut [u8]) {
        self.algo.decrypt_block(input, output);
    }
}

/// ECB Decryption mode
pub struct EcbDecryptor<T, X> {
    block_engine: BlockEngine<EcbDecryptorProcessor<T>, X>
}

impl <T: BlockDecryptor, X: PaddingProcessor> EcbDecryptor<T, X> {
    /// Create a new ECB decryption mode object
    pub fn new(algo: T, padding: X) -> EcbDecryptor<T, DecPadding<X>> {
        let block_size = algo.block_size();
        let processor = EcbDecryptorProcessor {
            algo: algo
        };
        EcbDecryptor {
            block_engine: BlockEngine::new(processor, DecPadding::wrap(padding), block_size)
        }
    }
    pub fn reset(&mut self) {
        self.block_engine.reset();
    }
}

impl <T: BlockDecryptor, X: PaddingProcessor> Decryptor for EcbDecryptor<T, X> {
    fn decrypt(&mut self, input: &mut RefReadBuffer, output: &mut RefWriteBuffer, eof: bool)
            -> Result<BufferResult, SymmetricCipherError> {
        self.block_engine.process(input, output, eof)
    }
}

struct CbcEncryptorProcessor<T> {
    algo: T,
    temp: Vec<u8>
}

impl <T: BlockEncryptor> BlockProcessor for CbcEncryptorProcessor<T> {
    fn process_block(&mut self, _: &[u8], out_hist: &[u8], input: &[u8], output: &mut [u8]) {
        for ((&x, &y), o) in input.iter().zip(out_hist.iter()).zip(self.temp.iter_mut()) {
            *o = x ^ y;
        }
        self.algo.encrypt_block(&self.temp[..], output);
    }
}

/// CBC encryption mode
pub struct CbcEncryptor<T, X> {
    block_engine: BlockEngine<CbcEncryptorProcessor<T>, X>
}

impl <T: BlockEncryptor, X: PaddingProcessor> CbcEncryptor<T, X> {
    /// Create a new CBC encryption mode object
    pub fn new(algo: T, padding: X, iv: Vec<u8>) -> CbcEncryptor<T, EncPadding<X>> {
        let block_size = algo.block_size();
        let processor = CbcEncryptorProcessor {
            algo: algo,
            temp: repeat(0).take(block_size).collect()
        };
        CbcEncryptor {
            block_engine: BlockEngine::new_with_history(
                processor,
                EncPadding::wrap(padding),
                block_size,
                Vec::new(),
                iv)
        }
    }
    pub fn reset(&mut self, iv: &[u8]) {
        self.block_engine.reset_with_history(&[], iv);
    }
}

impl <T: BlockEncryptor, X: PaddingProcessor> Encryptor for CbcEncryptor<T, X> {
    fn encrypt(&mut self, input: &mut RefReadBuffer, output: &mut RefWriteBuffer, eof: bool)
            -> Result<BufferResult, SymmetricCipherError> {
        self.block_engine.process(input, output, eof)
    }
}

struct CbcDecryptorProcessor<T> {
    algo: T,
    temp: Vec<u8>
}

impl <T: BlockDecryptor> BlockProcessor for CbcDecryptorProcessor<T> {
    fn process_block(&mut self, in_hist: &[u8], _: &[u8], input: &[u8], output: &mut [u8]) {
        self.algo.decrypt_block(input, &mut self.temp);
        for ((&x, &y), o) in self.temp.iter().zip(in_hist.iter()).zip(output.iter_mut()) {
            *o = x ^ y;
        }
    }
}

/// CBC decryption mode
pub struct CbcDecryptor<T, X> {
    block_engine: BlockEngine<CbcDecryptorProcessor<T>, X>
}

impl <T: BlockDecryptor, X: PaddingProcessor> CbcDecryptor<T, X> {
    /// Create a new CBC decryption mode object
    pub fn new(algo: T, padding: X, iv: Vec<u8>) -> CbcDecryptor<T, DecPadding<X>> {
        let block_size = algo.block_size();
        let processor = CbcDecryptorProcessor {
            algo: algo,
            temp: repeat(0).take(block_size).collect()
        };
        CbcDecryptor {
            block_engine: BlockEngine::new_with_history(
                processor,
                DecPadding::wrap(padding),
                block_size,
                iv,
                Vec::new())
        }
    }
    pub fn reset(&mut self, iv: &[u8]) {
        self.block_engine.reset_with_history(iv, &[]);
    }
}

impl <T: BlockDecryptor, X: PaddingProcessor> Decryptor for CbcDecryptor<T, X> {
    fn decrypt(&mut self, input: &mut RefReadBuffer, output: &mut RefWriteBuffer, eof: bool)
            -> Result<BufferResult, SymmetricCipherError> {
        self.block_engine.process(input, output, eof)
    }
}

fn add_ctr(ctr: &mut [u8], mut ammount: u8) {
    for i in ctr.iter_mut().rev() {
        let prev = *i;
        *i = i.wrapping_add(ammount);
        if *i >= prev {
            break;
        }
        ammount = 1;
    }
}

/// CTR Mode
pub struct CtrMode<A> {
    algo: A,
    ctr: Vec<u8>,
    bytes: OwnedReadBuffer
}

impl <A: BlockEncryptor> CtrMode<A> {
    /// Create a new CTR object
    pub fn new(algo: A, ctr: Vec<u8>) -> CtrMode<A> {
        let block_size = algo.block_size();
        CtrMode {
            algo: algo,
            ctr: ctr,
            bytes: OwnedReadBuffer::new_with_len(repeat(0).take(block_size).collect(), 0)
        }
    }
    pub fn reset(&mut self, ctr: &[u8]) {
        cryptoutil::copy_memory(ctr, &mut self.ctr);
        self.bytes.reset();
    }
    fn process(&mut self, input: &[u8], output: &mut [u8]) {
        assert!(input.len() == output.len());
        let len = input.len();
        let mut i = 0;
        while i < len {
            if self.bytes.is_empty() {
                let mut wb = self.bytes.borrow_write_buffer();
                self.algo.encrypt_block(&self.ctr[..], wb.take_remaining());
                add_ctr(&mut self.ctr, 1);
            }
            let count = cmp::min(self.bytes.remaining(), len - i);
            let bytes_it = self.bytes.take_next(count).iter();
            let in_it = input[i..].iter();
            let out_it = output[i..].iter_mut();
            for ((&x, &y), o) in bytes_it.zip(in_it).zip(out_it) {
                *o = x ^ y;
            }
            i += count;
        }
    }
}

impl <A: BlockEncryptor> SynchronousStreamCipher for CtrMode<A> {
    fn process(&mut self, input: &[u8], output: &mut [u8]) {
        self.process(input, output);
    }
}

impl <A: BlockEncryptor> Encryptor for CtrMode<A> {
    fn encrypt(&mut self, input: &mut RefReadBuffer, output: &mut RefWriteBuffer, _: bool)
            -> Result<BufferResult, SymmetricCipherError> {
        symm_enc_or_dec(self, input, output)
    }
}

impl <A: BlockEncryptor> Decryptor for CtrMode<A> {
    fn decrypt(&mut self, input: &mut RefReadBuffer, output: &mut RefWriteBuffer, _: bool)
            -> Result<BufferResult, SymmetricCipherError> {
        symm_enc_or_dec(self, input, output)
    }
}

/// CTR Mode that operates on 8 blocks at a time
pub struct CtrModeX8<A> {
    algo: A,
    ctr_x8: Vec<u8>,
    bytes: OwnedReadBuffer
}

fn construct_ctr_x8(in_ctr: &[u8], out_ctr_x8: &mut [u8]) {
    for (i, ctr_i) in out_ctr_x8.chunks_mut(in_ctr.len()).enumerate() {
        cryptoutil::copy_memory(in_ctr, ctr_i);
        add_ctr(ctr_i, i as u8);
    }
}

impl <A: BlockEncryptorX8> CtrModeX8<A> {
    /// Create a new CTR object that operates on 8 blocks at a time
    pub fn new(algo: A, ctr: &[u8]) -> CtrModeX8<A> {
        let block_size = algo.block_size();
        let mut ctr_x8: Vec<u8> = repeat(0).take(block_size * 8).collect();
        construct_ctr_x8(ctr, &mut ctr_x8);
        CtrModeX8 {
            algo: algo,
            ctr_x8: ctr_x8,
            bytes: OwnedReadBuffer::new_with_len(repeat(0).take(block_size * 8).collect(), 0)
        }
    }
    pub fn reset(&mut self, ctr: &[u8]) {
        construct_ctr_x8(ctr, &mut self.ctr_x8);
        self.bytes.reset();
    }
    fn process(&mut self, input: &[u8], output: &mut [u8]) {
        // TODO - Can some of this be combined with regular CtrMode?
        assert!(input.len() == output.len());
        let len = input.len();
        let mut i = 0;
        while i < len {
            if self.bytes.is_empty() {
                let mut wb = self.bytes.borrow_write_buffer();
                self.algo.encrypt_block_x8(&self.ctr_x8[..], wb.take_remaining());
                for ctr_i in &mut self.ctr_x8.chunks_mut(self.algo.block_size()) {
                    add_ctr(ctr_i, 8);
                }
            }
            let count = cmp::min(self.bytes.remaining(), len - i);
            let bytes_it = self.bytes.take_next(count).iter();
            let in_it = input[i..].iter();
            let out_it = &mut output[i..];
            for ((&x, &y), o) in bytes_it.zip(in_it).zip(out_it.iter_mut()) {
                *o = x ^ y;
            }
            i += count;
        }
    }
}

impl <A: BlockEncryptorX8> SynchronousStreamCipher for CtrModeX8<A> {
    fn process(&mut self, input: &[u8], output: &mut [u8]) {
        self.process(input, output);
    }
}

impl <A: BlockEncryptorX8> Encryptor for CtrModeX8<A> {
    fn encrypt(&mut self, input: &mut RefReadBuffer, output: &mut RefWriteBuffer, _: bool)
            -> Result<BufferResult, SymmetricCipherError> {
        symm_enc_or_dec(self, input, output)
    }
}

impl <A: BlockEncryptorX8> Decryptor for CtrModeX8<A> {
    fn decrypt(&mut self, input: &mut RefReadBuffer, output: &mut RefWriteBuffer, _: bool)
            -> Result<BufferResult, SymmetricCipherError> {
        symm_enc_or_dec(self, input, output)
    }
}

#[cfg(test)]
mod test {
    use std::iter::repeat;

    use aessafe;
    use blockmodes::{EcbEncryptor, EcbDecryptor, CbcEncryptor, CbcDecryptor, CtrMode, CtrModeX8,
        NoPadding, PkcsPadding};
    use buffer::{ReadBuffer, WriteBuffer, RefReadBuffer, RefWriteBuffer, BufferResult};
    use buffer::BufferResult::{BufferUnderflow, BufferOverflow};
    use symmetriccipher::{Encryptor, Decryptor};
    use symmetriccipher::SymmetricCipherError::{self, InvalidLength, InvalidPadding};

    use std::cmp;

    trait CipherTest {
        fn get_plain<'a>(&'a self) -> &'a [u8];
        fn get_cipher<'a>(&'a self) -> &'a [u8];
    }

    struct EcbTest {
        key: Vec<u8>,
        plain: Vec<u8>,
        cipher: Vec<u8>
    }

    impl CipherTest for EcbTest {
        fn get_plain<'a>(&'a self) -> &'a [u8] {
            &self.plain[..]
        }
        fn get_cipher<'a>(&'a self) -> &'a [u8] {
            &self.cipher[..]
        }
    }

    struct CbcTest {
        key: Vec<u8>,
        iv: Vec<u8>,
        plain: Vec<u8>,
        cipher: Vec<u8>
    }

    impl CipherTest for CbcTest {
        fn get_plain<'a>(&'a self) -> &'a [u8] {
            &self.plain[..]
        }
        fn get_cipher<'a>(&'a self) -> &'a [u8] {
            &self.cipher[..]
        }
    }

    struct CtrTest {
        key: Vec<u8>,
        ctr: Vec<u8>,
        plain: Vec<u8>,
        cipher: Vec<u8>
    }

    impl CipherTest for CtrTest {
        fn get_plain<'a>(&'a self) -> &'a [u8] {
            &self.plain[..]
        }
        fn get_cipher<'a>(&'a self) -> &'a [u8] {
            &self.cipher[..]
        }
    }

    fn aes_ecb_no_padding_tests() -> Vec<EcbTest> {
        vec![
            EcbTest {
                key: repeat(0).take(16).collect(),
                plain: repeat(0).take(32).collect(),
                cipher: vec![
                    0x66, 0xe9, 0x4b, 0xd4, 0xef, 0x8a, 0x2c, 0x3b,
                    0x88, 0x4c, 0xfa, 0x59, 0xca, 0x34, 0x2b, 0x2e,
                    0x66, 0xe9, 0x4b, 0xd4, 0xef, 0x8a, 0x2c, 0x3b,
                    0x88, 0x4c, 0xfa, 0x59, 0xca, 0x34, 0x2b, 0x2e ]
            }
        ]
    }

    fn aes_ecb_pkcs_padding_tests() -> Vec<EcbTest> {
        vec![
            EcbTest {
                key: repeat(0).take(16).collect(),
                plain: repeat(0).take(32).collect(),
                cipher: vec![
                    0x66, 0xe9, 0x4b, 0xd4, 0xef, 0x8a, 0x2c, 0x3b,
                    0x88, 0x4c, 0xfa, 0x59, 0xca, 0x34, 0x2b, 0x2e,
                    0x66, 0xe9, 0x4b, 0xd4, 0xef, 0x8a, 0x2c, 0x3b,
                    0x88, 0x4c, 0xfa, 0x59, 0xca, 0x34, 0x2b, 0x2e,
                    0x01, 0x43, 0xdb, 0x63, 0xee, 0x66, 0xb0, 0xcd,
                    0xff, 0x9f, 0x69, 0x91, 0x76, 0x80, 0x15, 0x1e ]
            },
            EcbTest {
                key: repeat(0).take(16).collect(),
                plain: repeat(0).take(33).collect(),
                cipher: vec![
                    0x66, 0xe9, 0x4b, 0xd4, 0xef, 0x8a, 0x2c, 0x3b,
                    0x88, 0x4c, 0xfa, 0x59, 0xca, 0x34, 0x2b, 0x2e,
                    0x66, 0xe9, 0x4b, 0xd4, 0xef, 0x8a, 0x2c, 0x3b,
                    0x88, 0x4c, 0xfa, 0x59, 0xca, 0x34, 0x2b, 0x2e,
                    0x7a, 0xdc, 0x99, 0xb2, 0x9e, 0x82, 0xb1, 0xb2,
                    0xb0, 0xa6, 0x5a, 0x38, 0xbc, 0x57, 0x8a, 0x01 ]
            }
        ]
    }

    fn aes_cbc_no_padding_tests() -> Vec<CbcTest> {
        vec![
            CbcTest {
                key: repeat(1).take(16).collect(),
                iv: repeat(3).take(16).collect(),
                plain: repeat(2).take(32).collect(),
                cipher: vec![
                    0x5e, 0x77, 0xe5, 0x9f, 0x8f, 0x85, 0x94, 0x34,
                    0x89, 0xa2, 0x41, 0x49, 0xc7, 0x5f, 0x4e, 0xc9,
                    0xe0, 0x9a, 0x77, 0x36, 0xfb, 0xc8, 0xb2, 0xdc,
                    0xb3, 0xfb, 0x9f, 0xc0, 0x31, 0x4c, 0xb0, 0xb1 ]
            }
        ]
    }

    fn aes_cbc_pkcs_padding_tests() -> Vec<CbcTest> {
        vec![
            CbcTest {
                key: repeat(1).take(16).collect(),
                iv: repeat(3).take(16).collect(),
                plain: repeat(2).take(32).collect(),
                cipher: vec![
                    0x5e, 0x77, 0xe5, 0x9f, 0x8f, 0x85, 0x94, 0x34,
                    0x89, 0xa2, 0x41, 0x49, 0xc7, 0x5f, 0x4e, 0xc9,
                    0xe0, 0x9a, 0x77, 0x36, 0xfb, 0xc8, 0xb2, 0xdc,
                    0xb3, 0xfb, 0x9f, 0xc0, 0x31, 0x4c, 0xb0, 0xb1,
                    0xa4, 0xc2, 0xe4, 0x62, 0xef, 0x7a, 0xe3, 0x7e,
                    0xef, 0x88, 0xf3, 0x27, 0xbd, 0x9c, 0xc8, 0x4d ]
            },
            CbcTest {
                key: repeat(1).take(16).collect(),
                iv: repeat(3).take(16).collect(),
                plain: repeat(2).take(33).collect(),
                cipher: vec![
                    0x5e, 0x77, 0xe5, 0x9f, 0x8f, 0x85, 0x94, 0x34,
                    0x89, 0xa2, 0x41, 0x49, 0xc7, 0x5f, 0x4e, 0xc9,
                    0xe0, 0x9a, 0x77, 0x36, 0xfb, 0xc8, 0xb2, 0xdc,
                    0xb3, 0xfb, 0x9f, 0xc0, 0x31, 0x4c, 0xb0, 0xb1,
                    0x6c, 0x47, 0xcd, 0xec, 0xae, 0xbb, 0x1a, 0x65,
                    0x04, 0xd2, 0x32, 0x23, 0xa6, 0x8d, 0x4a, 0x65 ]
            }
        ]
    }

    fn aes_ctr_tests() -> Vec<CtrTest> {
        vec![
            CtrTest {
                key: repeat(1).take(16).collect(),
                ctr: repeat(3).take(16).collect(),
                plain: repeat(2).take(33).collect(),
                cipher: vec![
                    0x64, 0x3e, 0x05, 0x19, 0x79, 0x78, 0xd7, 0x45,
                    0xa9, 0x10, 0x5f, 0xd8, 0x4c, 0xd7, 0xe6, 0xb1,
                    0x5f, 0x66, 0xc6, 0x17, 0x4b, 0x25, 0xea, 0x24,
                    0xe6, 0xf9, 0x19, 0x09, 0xb7, 0xdd, 0x84, 0xfb,
                    0x86 ]
            }
        ]
    }

    // Test the mode by encrypting all of the data at once
    fn run_full_test<T: CipherTest, E: Encryptor, D: Decryptor>(
            test: &T,
            enc: &mut E,
            dec: &mut D) {
        let mut cipher_out: Vec<u8> = repeat(0).take(test.get_cipher().len()).collect();
        {
            let mut buff_in = RefReadBuffer::new(test.get_plain());
            let mut buff_out = RefWriteBuffer::new(&mut cipher_out);
            match enc.encrypt(&mut buff_in, &mut buff_out, true) {
                Ok(BufferUnderflow) => {}
                Ok(BufferOverflow) => panic!("Encryption not completed"),
                Err(_) => panic!("Error"),
            }
        }
        assert!(test.get_cipher() == &cipher_out[..]);

        let mut plain_out: Vec<u8> = repeat(0).take(test.get_plain().len()).collect();
        {
            let mut buff_in = RefReadBuffer::new(test.get_cipher());
            let mut buff_out = RefWriteBuffer::new(&mut plain_out);
            match dec.decrypt(&mut buff_in, &mut buff_out, true) {
                Ok(BufferUnderflow) => {}
                Ok(BufferOverflow) => panic!("Decryption not completed"),
                Err(_) => panic!("Error"),
            }
        }
        assert!(test.get_plain() == &plain_out[..]);
    }

    /// Run and encryption or decryption operation, passing in variable sized input and output
    /// buffers.
    ///
    /// # Arguments
    /// * input - The complete input vector
    /// * output - The complete output vector
    /// * op - A closure that runs either an encryption or decryption operation
    /// * next_in_len - A closure that returns the length to use for the next input buffer; If the
    ///                 returned value is not in a valid range, it will be fixed up by this
    ///                 function.
    /// * next_out_len - A closure that returns the length to use for the next output buffer; If the
    ///                  returned value is not in a valid range, it will be fixed up by this
    ///                  function.
    /// * immediate_eof - Whether eof should be set immediately upon running out of input or if eof
    ///                   should be passed only after all input has been consumed.
    fn run_inc<OpFunc, NextInFunc, NextOutFunc>(
            input: &[u8],
            output: &mut [u8],
            mut op: OpFunc,
            mut next_in_len: NextInFunc,
            mut next_out_len: NextOutFunc,
            immediate_eof: bool)
            where
                OpFunc: FnMut(&mut RefReadBuffer, &mut RefWriteBuffer, bool) ->
                    Result<BufferResult, SymmetricCipherError>,
                NextInFunc: FnMut() -> usize,
                NextOutFunc: FnMut() -> usize {
        use std::cell::Cell;

        let in_len = input.len();
        let out_len = output.len();

        let mut state: Result<BufferResult, SymmetricCipherError> = Ok(BufferUnderflow);
        let mut in_pos: usize = 0;
        let mut out_pos: usize = 0;
        let eof = Cell::new(false);

        let mut in_end = |in_pos: usize, primary: bool| {
            if eof.get() {
                return in_len;
            }
            let x = next_in_len();
            if x >= in_len && immediate_eof {
                eof.set(true);
            }
            cmp::min(in_len, in_pos + cmp::max(x, if primary { 1 } else { 0 }))
        };

        let mut out_end = |out_pos: usize| {
            let x = next_out_len();
            cmp::min(out_len, out_pos + cmp::max(x, 1))
        };

        loop {
            match state {
                Ok(BufferUnderflow) => {
                    if in_pos == input.len() {
                        break;
                    }
                    let mut tmp_in = RefReadBuffer::new(&input[in_pos..in_end(in_pos, true)]);
                    let out_end = out_end(out_pos);
                    let mut tmp_out = RefWriteBuffer::new(&mut output[out_pos..out_end]);
                    state = op(&mut tmp_in, &mut tmp_out, eof.get());
                    match state {
                        Ok(BufferUnderflow) => assert!(tmp_in.is_empty()),
                        _ => {}
                    }
                    in_pos += tmp_in.position();
                    out_pos += tmp_out.position();
                }
                Ok(BufferOverflow) => {
                    let mut tmp_in = RefReadBuffer::new(&input[in_pos..in_end(in_pos, false)]);
                    let out_end = out_end(out_pos);
                    let mut tmp_out = RefWriteBuffer::new(&mut output[out_pos..out_end]);
                    state = op(&mut tmp_in, &mut tmp_out, eof.get());
                    match state {
                        Ok(BufferOverflow) => assert!(tmp_out.is_full()),
                        _ => {}
                    }
                    in_pos += tmp_in.position();
                    out_pos += tmp_out.position();
                }
                Err(InvalidPadding) => panic!("Invalid Padding"),
                Err(InvalidLength) => panic!("Invalid Length")
            }
        }

        if !eof.get() {
            eof.set(true);
            let mut tmp_out = RefWriteBuffer::new(&mut output[out_pos..out_end(out_pos)]);
            state = op(&mut RefReadBuffer::new(&[]), &mut tmp_out, eof.get());
            out_pos += tmp_out.position();
        }

        loop {
            match state {
                Ok(BufferUnderflow) => {
                    break;
                }
                Ok(BufferOverflow) => {
                    let out_end = out_end(out_pos);
                    let mut tmp_out = RefWriteBuffer::new(&mut output[out_pos..out_end]);
                    state = op(&mut RefReadBuffer::new(&[]), &mut tmp_out, eof.get());
                    assert!(tmp_out.is_full());
                    out_pos += tmp_out.position();
                }
                Err(InvalidPadding) => panic!("Invalid Padding"),
                Err(InvalidLength) => panic!("Invalid Length")
            }
        }
    }

    fn run_inc1_test<T: CipherTest, E: Encryptor, D: Decryptor>(
            test: &T,
            enc: &mut E,
            dec: &mut D) {
        let mut cipher_out: Vec<u8> = repeat(0).take(test.get_cipher().len()).collect();
        run_inc(
            test.get_plain(),
            &mut cipher_out,
            |in_buff: &mut RefReadBuffer, out_buff: &mut RefWriteBuffer, eof: bool| {
                enc.encrypt(in_buff, out_buff, eof)
            },
            || { 0 },
            || { 1 },
            false);
        assert!(test.get_cipher() == &cipher_out[..]);

        let mut plain_out: Vec<u8> = repeat(0).take(test.get_plain().len()).collect();
        run_inc(
            test.get_cipher(),
            &mut plain_out,
            |in_buff: &mut RefReadBuffer, out_buff: &mut RefWriteBuffer, eof: bool| {
                dec.decrypt(in_buff, out_buff, eof)
            },
            || { 0 },
            || { 1 },
            false);
        assert!(test.get_plain() == &plain_out[..]);
    }

    fn run_rand_test<T, E, D, NewEncFunc, NewDecFunc>(
            test: &T,
            mut new_enc: NewEncFunc,
            mut new_dec: NewDecFunc)
            where
                T: CipherTest,
                E: Encryptor,
                D: Decryptor,
                NewEncFunc: FnMut() -> E,
                NewDecFunc: FnMut() -> D{
        use rand;
        use rand::Rng;

        let tmp : &[_] = &[1, 2, 3, 4];
        let mut rng1: rand::StdRng = rand::SeedableRng::from_seed(tmp);
        let mut rng2: rand::StdRng = rand::SeedableRng::from_seed(tmp);
        let mut rng3: rand::StdRng = rand::SeedableRng::from_seed(tmp);
        let max_size = cmp::max(test.get_plain().len(), test.get_cipher().len());

        let mut r1 = || {
            rng1.gen_range(0, max_size)
        };
        let mut r2 = || {
            rng2.gen_range(0, max_size)
        };

        for _ in 0..1000 {
            let mut enc = new_enc();
            let mut dec = new_dec();

            let mut cipher_out: Vec<u8> = repeat(0).take(test.get_cipher().len()).collect();
            run_inc(
                test.get_plain(),
                &mut cipher_out,
                |in_buff: &mut RefReadBuffer, out_buff: &mut RefWriteBuffer, eof: bool| {
                    enc.encrypt(in_buff, out_buff, eof)
                },
                || { r1() },
                || { r2() },
                rng3.gen());
            assert!(test.get_cipher() == &cipher_out[..]);

            let mut plain_out: Vec<u8> = repeat(0).take(test.get_plain().len()).collect();
            run_inc(
                test.get_cipher(),
                &mut plain_out,
                |in_buff: &mut RefReadBuffer, out_buff: &mut RefWriteBuffer, eof: bool| {
                    dec.decrypt(in_buff, out_buff, eof)
                },
                || { r1() },
                || { r2() },
                rng3.gen());
            assert!(test.get_plain() == &plain_out[..]);
        }
    }

    fn run_test<T, E, D, NewEncFunc, NewDecFunc>(
            test: &T,
            mut new_enc: NewEncFunc,
            mut new_dec: NewDecFunc)
            where
                T: CipherTest,
                E: Encryptor,
                D: Decryptor,
                NewEncFunc: FnMut() -> E,
                NewDecFunc: FnMut() -> D{
        run_full_test(test, &mut new_enc(), &mut new_dec());
        run_inc1_test(test, &mut new_enc(), &mut new_dec());
        run_rand_test(test, new_enc, new_dec);
    }

    #[test]
    fn aes_ecb_no_padding() {
        let tests = aes_ecb_no_padding_tests();
        for test in tests.iter() {
            run_test(
                test,
                || {
                    let aes_enc = aessafe::AesSafe128Encryptor::new(&test.key[..]);
                    EcbEncryptor::new(aes_enc, NoPadding)
                },
                || {
                    let aes_dec = aessafe::AesSafe128Decryptor::new(&test.key[..]);
                    EcbDecryptor::new(aes_dec, NoPadding)
                });
        }
    }

    #[test]
    fn aes_ecb_pkcs_padding() {
        let tests = aes_ecb_pkcs_padding_tests();
        for test in tests.iter() {
            run_test(
                test,
                || {
                    let aes_enc = aessafe::AesSafe128Encryptor::new(&test.key[..]);
                    EcbEncryptor::new(aes_enc, PkcsPadding)
                },
                || {
                    let aes_dec = aessafe::AesSafe128Decryptor::new(&test.key[..]);
                    EcbDecryptor::new(aes_dec, PkcsPadding)
                });
        }
    }

    #[test]
    fn aes_cbc_no_padding() {
        let tests = aes_cbc_no_padding_tests();
        for test in tests.iter() {
            run_test(
                test,
                || {
                    let aes_enc = aessafe::AesSafe128Encryptor::new(&test.key[..]);
                    CbcEncryptor::new(aes_enc, NoPadding, test.iv.clone())
                },
                || {
                    let aes_dec = aessafe::AesSafe128Decryptor::new(&test.key[..]);
                    CbcDecryptor::new(aes_dec, NoPadding, test.iv.clone())
                });
        }
    }

    #[test]
    fn aes_cbc_pkcs_padding() {
        let tests = aes_cbc_pkcs_padding_tests();
        for test in tests.iter() {
            run_test(
                test,
                || {
                    let aes_enc = aessafe::AesSafe128Encryptor::new(&test.key[..]);
                    CbcEncryptor::new(aes_enc, PkcsPadding, test.iv.clone())
                },
                || {
                    let aes_dec = aessafe::AesSafe128Decryptor::new(&test.key[..]);
                    CbcDecryptor::new(aes_dec, PkcsPadding, test.iv.clone())
                });
        }
    }

    #[test]
    fn aes_ctr() {
        let tests = aes_ctr_tests();
        for test in tests.iter() {
            run_test(
                test,
                || {
                    let aes_enc = aessafe::AesSafe128Encryptor::new(&test.key[..]);
                    CtrMode::new(aes_enc, test.ctr.clone())
                },
                || {
                    let aes_enc = aessafe::AesSafe128Encryptor::new(&test.key[..]);
                    CtrMode::new(aes_enc, test.ctr.clone())
                });
        }
    }

    #[test]
    fn aes_ctr_x8() {
        let tests = aes_ctr_tests();
        for test in tests.iter() {
            run_test(
                test,
                || {
                    let aes_enc = aessafe::AesSafe128EncryptorX8::new(&test.key[..]);
                    CtrModeX8::new(aes_enc, &test.ctr[..])
                },
                || {
                    let aes_enc = aessafe::AesSafe128EncryptorX8::new(&test.key[..]);
                    CtrModeX8::new(aes_enc, &test.ctr[..])
                });
        }
    }
}

#[cfg(all(test, feature = "with-bench"))]
mod bench {
    use aessafe;
    use blockmodes::{EcbEncryptor, CbcEncryptor, CtrMode, CtrModeX8,
        NoPadding, PkcsPadding};
    use buffer::{ReadBuffer, WriteBuffer, RefReadBuffer, RefWriteBuffer};
    use buffer::BufferResult::{BufferUnderflow, BufferOverflow};
    use symmetriccipher::{Encryptor};

    use test::Bencher;

    #[bench]
    pub fn aes_ecb_no_padding_bench(bh: &mut Bencher) {
        let key = [1u8; 16];
        let plain = [3u8; 512];
        let mut cipher = [3u8; 528];

        let aes_enc = aessafe::AesSafe128Encryptor::new(&key);
        let mut enc = EcbEncryptor::new(aes_enc, NoPadding);

        bh.iter( || {
            enc.reset();

            let mut buff_in = RefReadBuffer::new(&plain);
            let mut buff_out = RefWriteBuffer::new(&mut cipher);

            match enc.encrypt(&mut buff_in, &mut buff_out, true) {
                Ok(BufferUnderflow) => {}
                Ok(BufferOverflow) => panic!("Encryption not completed"),
                Err(_) => panic!("Error"),
            }
        });

        bh.bytes = (plain.len()) as u64;
    }

    #[bench]
    pub fn aes_cbc_pkcs_padding_bench(bh: &mut Bencher) {
        let key = [1u8; 16];
        let iv = [2u8; 16];
        let plain = [3u8; 512];
        let mut cipher = [3u8; 528];

        let aes_enc = aessafe::AesSafe128Encryptor::new(&key);
        let mut enc = CbcEncryptor::new(aes_enc, PkcsPadding, iv.to_vec());

        bh.iter( || {
            enc.reset(&iv);

            let mut buff_in = RefReadBuffer::new(&plain);
            let mut buff_out = RefWriteBuffer::new(&mut cipher);

            match enc.encrypt(&mut buff_in, &mut buff_out, true) {
                Ok(BufferUnderflow) => {}
                Ok(BufferOverflow) => panic!("Encryption not completed"),
                Err(_) => panic!("Error"),
            }
        });

        bh.bytes = (plain.len()) as u64;
    }

    #[bench]
    pub fn aes_ctr_bench(bh: &mut Bencher) {
        let key = [1u8; 16];
        let ctr = [2u8; 16];
        let plain = [3u8; 512];
        let mut cipher = [3u8; 528];

        let aes_enc = aessafe::AesSafe128Encryptor::new(&key);
        let mut enc = CtrMode::new(aes_enc, ctr.to_vec());

        bh.iter( || {
            enc.reset(&ctr);

            let mut buff_in = RefReadBuffer::new(&plain);
            let mut buff_out = RefWriteBuffer::new(&mut cipher);

            match enc.encrypt(&mut buff_in, &mut buff_out, true) {
                Ok(BufferUnderflow) => {}
                Ok(BufferOverflow) => panic!("Encryption not completed"),
                Err(_) => panic!("Error"),
            }
        });

        bh.bytes = (plain.len()) as u64;
    }

    #[bench]
    pub fn aes_ctr_x8_bench(bh: &mut Bencher) {
        let key = [1u8; 16];
        let ctr = [2u8; 16];
        let plain = [3u8; 512];
        let mut cipher = [3u8; 528];

        let aes_enc = aessafe::AesSafe128EncryptorX8::new(&key);
        let mut enc = CtrModeX8::new(aes_enc, &ctr);

        bh.iter( || {
            enc.reset(&ctr);

            let mut buff_in = RefReadBuffer::new(&plain);
            let mut buff_out = RefWriteBuffer::new(&mut cipher);

            match enc.encrypt(&mut buff_in, &mut buff_out, true) {
                Ok(BufferUnderflow) => {}
                Ok(BufferOverflow) => panic!("Encryption not completed"),
                Err(_) => panic!("Error"),
            }
        });

        bh.bytes = (plain.len()) as u64;
    }
}
