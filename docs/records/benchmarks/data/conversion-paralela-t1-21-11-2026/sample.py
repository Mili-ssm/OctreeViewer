#!/usr/bin/env python3
"""Sample /proc/stat, kernel-thread CPU, PSI io and loadavg every second until killed.
Usage: sample.py OUT.tsv   (columns: unix_s, cpu_total_ticks, cpu_system_ticks, cpu_iowait_ticks,
kthread_ticks, psi_io_some_total_us, psi_io_full_total_us, loadavg1)"""
import os, sys, time, re
KT = re.compile(r"^(kworker|btrfs|kswapd|flush|kcompactd|jbd2|nvme|irq/)")
def kthreads():
    total = 0
    for pid in os.listdir("/proc"):
        if not pid.isdigit():
            continue
        try:
            for tid in os.listdir(f"/proc/{pid}/task"):
                with open(f"/proc/{pid}/task/{tid}/stat") as f:
                    s = f.read()
                comm = s[s.index("(") + 1 : s.rindex(")")]
                if KT.match(comm):
                    rest = s[s.rindex(")") + 2 :].split()
                    total += int(rest[11]) + int(rest[12])
        except (OSError, ValueError):
            pass
    return total
def main(out):
    with open(out, "w") as o:
        while True:
            c = open("/proc/stat").readline().split()[1:]
            v = list(map(int, c))
            psi = {}
            for line in open("/proc/pressure/io"):
                p = line.split()
                psi[p[0]] = int(p[4].split("=")[1])
            o.write(f"{time.time():.3f}\t{sum(v)}\t{v[2]}\t{v[4]}\t{kthreads()}\t{psi['some']}\t{psi['full']}\t{open('/proc/loadavg').read().split()[0]}\n")
            o.flush()
            time.sleep(1)
main(sys.argv[1])
