# Copyright 2016 Joseph Birr-Pixton.
#
# Permission to use, copy, modify, and/or distribute this software for any
# purpose with or without fee is hereby granted, provided that the above
# copyright notice and this permission notice appear in all copies.
#
# THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHORS DISCLAIM ALL WARRANTIES
# WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
# MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHORS BE LIABLE FOR
# ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
# WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
# ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
# OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

import subprocess
import glob
import hashlib
import os

TOP = '../../../../../'

def dump(bin, type):
    return '-----BEGIN %s-----\n%s-----END %s-----\n' % \
            (type, bin.encode('base64'), type)

def gen(outfile, paramfile, hashfn):
    param = open(paramfile).read()

    rand = os.urandom(64)
    hash = getattr(hashlib, hashfn)(rand).digest()

    proc = subprocess.Popen(['openssl', 'pkeyutl',
        '-inkey', 'priv.pem',
        '-sign',
        '-pkeyopt', 'rsa_padding_mode:pss',
        '-pkeyopt', 'rsa_pss_saltlen:-1',
        '-pkeyopt', 'digest:%s' % hashfn
        ],
        stdout = subprocess.PIPE,
        stdin = subprocess.PIPE)

    sig, _ = proc.communicate(hash)

    with open(outfile, 'w') as f:
        print >>f, dump(open('pub.der').read(), 'PUBLIC KEY')
        print >>f, dump(param, 'ALGORITHM')
        print >>f, dump(rand, 'DATA')

        assert len(sig) == 256 # only works with 2048-bit keys
        # turn it into a DER bitstring
        print >>f, dump('\x03\x82\x01\x01\x00' + sig, 'SIGNATURE')

if __name__ == '__main__':
    subprocess.check_call('openssl genrsa -out priv.pem 2048', shell = True)
    subprocess.check_call('openssl rsa -pubout -out pub.pem -in priv.pem', shell = True)
    subprocess.check_call('openssl asn1parse -inform pem -in pub.pem -out pub.der', shell = True)
    gen('rsa-pss-sha256-salt32.pem', TOP + 'src/data/alg-pss-sha256.der', 'sha256')
    gen('rsa-pss-sha384-salt48.pem', TOP + 'src/data/alg-pss-sha384.der', 'sha384')
    gen('rsa-pss-sha512-salt64.pem', TOP + 'src/data/alg-pss-sha512.der', 'sha512')
