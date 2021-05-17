Version v0.2.35 (4/4/2016)
==========================

* Improve cross compiling from GCC to msvc.
* Fix BCrypt algorithm when using a cost of 31.
* Improve building on OpenBSD.
* Add implementation of SHA3 digest function.
* Fix errors in Blake2b that could lead to incorrect output. The Blake2b
  initialization functions are modified to take parameters by value instead of
  by reference, which may break users of the interfaces.
