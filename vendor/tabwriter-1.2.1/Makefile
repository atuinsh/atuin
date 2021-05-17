all:
	echo Nothing to do...

ctags:
	ctags --recurse --options=ctags.rust --languages=Rust

docs:
	cargo doc
	in-dir ./target/doc fix-perms
	rscp ./target/doc/* gopher:~/www/burntsushi.net/rustdoc/

push:
	git push origin master
	git push github master

