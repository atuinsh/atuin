/*! `bitvec` symbol export

This module collects the general public API into a single spot for inclusion, as
`use bitvec::prelude::*;`, without polluting the root namespace of the crate.

The prelude has a number of submodules, which can be used to limit the symbols
imported.

The `base` module (`use bitvec::prelude::base::*;`) imports only the data types
and macros needed to make direct use of the crate. It also imports trait
*methods* from `BitField` and `BitView`, without importing those trait names.

The `macros` module imports only the constructor macros.

The `traits` module imports the names of all traits in the crate.

The `types` module imports all data types in the crate.

You may alternatively wish to import the crate root, or this prelude, under a
shorter name, without bringing any other items into scope. The import statements

```rust,ignore
use bitvec as bv;
//  or
use bitvec::prelude as bv;
```

will make the crate symbols available under the `bv` namespace instead of the
longer `bitvec`. The prelude contains all the major public symbols of the crate
directly, while the crate root does not reëxport the items in its submodules.
Use whichever path root you prefer: crate for full paths, and prelude for
shortcuts.
!*/

/// The base symbols, containing only the minimum needed to use the crate.
pub mod base {
	pub use super::{
		macros::*,
		trait_methods::*,
	};

	pub use crate::{
		array::BitArray,
		order::{
			LocalBits,
			Lsb0,
			Msb0,
		},
		slice::BitSlice,
	};

	#[cfg(feature = "alloc")]
	pub use crate::{
		boxed::BitBox,
		vec::BitVec,
	};
}

/// Macros available for default export.
pub mod macros {
	pub use crate::{
		bitarr,
		bits,
	};

	#[cfg(feature = "alloc")]
	pub use crate::{
		bitbox,
		bitvec,
	};
}

/// Traits available for default export.
pub mod traits {
	pub use crate::{
		field::BitField,
		mem::BitMemory,
		order::BitOrder,
		store::BitStore,
		view::BitView,
	};
}

/// Imports trait methods without importing the traits themselves.
pub mod trait_methods {
	pub use crate::{
		field::BitField as _,
		view::BitView as _,
	};
}

/// Datatypes available for default export.
pub mod types {
	pub use crate::{
		array::BitArray,
		domain::{
			BitDomain,
			BitDomainMut,
		},
		order::{
			LocalBits,
			Lsb0,
			Msb0,
		},
		slice::BitSlice,
	};

	#[cfg(feature = "alloc")]
	pub use crate::{
		boxed::BitBox,
		vec::BitVec,
	};
}

pub use macros::*;
pub use traits::*;
pub use types::*;
