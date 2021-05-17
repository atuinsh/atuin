#! /bin/sh
# pwd
cargo fuzz run server_basic -- -dict=fuzz/fuzzer_dict -only_ascii=1 -timeout=60 ${FUZZ_LEN:+ -max_len=$FUZZ_LEN} $@
