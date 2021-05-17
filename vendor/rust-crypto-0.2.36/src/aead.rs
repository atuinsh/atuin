// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

pub trait AeadEncryptor {

	fn encrypt(&mut self, input: &[u8], output: &mut [u8], tag: &mut [u8]);
}

pub trait AeadDecryptor {

	fn decrypt(&mut self, input: &[u8], output: &mut [u8], tag: &[u8]) -> bool;
}