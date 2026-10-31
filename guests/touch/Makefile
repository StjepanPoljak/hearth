# A guest that writes to one page and stops, booted via PVH like Doom.
#
#   make            build touch.elf
#   make run        boot it (from the repository root's target/debug)
#   make trace      boot it under shadow-page-trace (sudo; build that
#                   first with its build.sh), printing what KVM does for
#                   each fault
#
# PAGING=0 on any of these builds touch-nopaging.elf instead, which makes
# the write with the guest's paging off.
#
# Build output goes to the repository's build/ directory.

ROOT	:= $(abspath ../..)
BUILD	:= $(ROOT)/build/touch
VMM	:= $(ROOT)/target/debug/hearth

CC	:= gcc
CFLAGS	:= -ffreestanding -nostdinc -fno-pie
LDFLAGS	:= -nostdlib -static -no-pie -T linker.ld -Wl,--build-id=none \
	-Wl,-z,max-page-size=0x1000 -Wl,-z,noexecstack

PAGING	?= 1
NAME	:= touch$(if $(filter 0,$(PAGING)),-nopaging)
ELF	:= $(BUILD)/$(NAME).elf

all: $(ELF)

$(ELF): $(BUILD)/$(NAME).o linker.ld
	$(CC) $(LDFLAGS) $< -o $@

$(BUILD)/$(NAME).o: boot.S | $(BUILD)
	$(CC) $(CFLAGS) -DPAGING=$(PAGING) -c $< -o $@

$(BUILD):
	mkdir -p $(BUILD)

RUN	:= $(VMM) --kernel $(ELF) --memory 4M

run: all
	$(RUN)

TRACER	:= $(ROOT)/shadow-page-trace/shadow-page-trace

trace: all
	sudo $(TRACER) -text -exec "$(RUN)"

clean:
	rm -rf $(BUILD)

.PHONY: all run trace clean
