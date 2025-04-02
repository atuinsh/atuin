#! /bin/sh

printf 'GET /healthz HTTP/1.1\n\n\n\n' | nc -q 10 localhost 8888 | sed -En 's/^.*[{,]"(status)":"([^"]*)".*$/\1: \2/p' | grep status || exit 1
