#! /bin/sh

printf 'GET / HTTP/1.1\n\n\n\n' | nc -q 10 localhost 8888 | sed -En 's/^.*,"(version)":"([^"]*)".*$/\1: \2/p' | grep version || exit 1
