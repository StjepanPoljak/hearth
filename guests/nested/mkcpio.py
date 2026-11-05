# mkcpio.py OUT ENTRY...: a newc initramfs without needing root.
# ENTRY is NAME=FILE (a file, mode 755), NAME/ (a directory) or
# NAME:c:MAJOR:MINOR (a character device).
import sys
def entry(out, name, mode, data=b"", rdev=(0, 0)):
    name = name.encode() + b"\0"
    hdr = "070701" + "".join("%08x" % v for v in (
        entry.ino, mode, 0, 0, 1, 0, len(data), 0, 0, rdev[0], rdev[1], len(name), 0))
    entry.ino += 1
    out.write(hdr.encode() + name + b"\0" * (-(110 + len(name)) % 4))
    out.write(data + b"\0" * (-len(data) % 4))
entry.ino = 1
with open(sys.argv[1], "wb") as out:
    for e in sys.argv[2:]:
        if e.endswith("/"):
            entry(out, e[:-1], 0o40755)
        elif ":c:" in e:
            name, _, major, minor = e.split(":")
            entry(out, name, 0o20600, rdev=(int(major), int(minor)))
        else:
            name, path = e.split("=", 1)
            entry(out, name, 0o100755, open(path, "rb").read())
    entry(out, "TRAILER!!!", 0)
