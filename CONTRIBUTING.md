# Contributing to Hearth

Thanks for wanting to help. Bug reports, ideas and code are all welcome.

## Your code stays yours

There is no contributor licence agreement to sign. When you contribute
code, you keep its copyright and license it under the AGPL-3.0, the same as
the rest of Hearth. You give up nothing else.

Hearth is not a commercial project and isn't meant to become one. If it
ever were licensed commercially, that would happen only with your
agreement for your code, and you would be paid a fair share. If you don't
agree, or can't be reached, your code would be rewritten before it went
into a commercial version.

By sending a contribution you confirm that you wrote it, or have the right
to submit it under the AGPL-3.0.

## Before you send a change

- Run `cargo test` and make sure it passes.
- If the change touches devices, memory or boot, also run the test guests
  in `guests/` (`guests/*/run.sh`). They need `/dev/kvm`, and some need
  nasm or QEMU; see the README and each script's header.
- Keep each change to one subject, with a commit message that says what
  it does and why.
