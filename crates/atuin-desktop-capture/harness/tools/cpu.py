"""Prints CPU milliseconds per command name for the processes of a session id."""
import os, sys, collections
sid = int(sys.argv[1]); tick = os.sysconf("SC_CLK_TCK"); tot = collections.Counter()
for p in os.listdir("/proc"):
    if not p.isdigit(): continue
    try: s = open(f"/proc/{p}/stat").read()
    except OSError: continue
    comm = s[s.index("(")+1:s.rindex(")")]; f = s[s.rindex(")")+2:].split()
    if int(f[3]) != sid: continue
    tot[comm] += (int(f[11]) + int(f[12])) * 1000 // tick
for k, v in sorted(tot.items()): print(f"{k}\t{v}")
