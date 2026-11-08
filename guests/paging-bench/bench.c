/*
 * /init of a Linux guest that times what shadow paging makes slow: new
 * address spaces (fork, exec), page faults, and switches between
 * processes, each a CR3 load. Prints one "bench:" line per test, then
 * reboots, which stops Hearth. Static, so the initramfs needs nothing else.
 */

#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include <sys/mman.h>
#include <sys/reboot.h>
#include <sys/wait.h>

static double now_us(void)
{
	struct timespec ts;
	clock_gettime(CLOCK_MONOTONIC, &ts);
	return ts.tv_sec * 1e6 + ts.tv_nsec / 1e3;
}

static void report(const char *name, long n, double start)
{
	double us = now_us() - start;
	printf("bench: %s %ld ops %.0f us %.2f us/op\n", name, n, us, us / n);
	fflush(stdout);
}

/* Each child execs this program again, which exits at once. */
static void fork_exec(long n)
{
	double start = now_us();
	for (long i = 0; i < n; i++) {
		pid_t pid = fork();
		if (pid == 0) {
			execl("/init", "init", "exit", (char *)NULL);
			_exit(127);
		}
		waitpid(pid, NULL, 0);
	}
	report("fork_exec", n, start);
}

/* The parent has 16 MiB written, so each fork copies its page tables and
 * the child's writes take copy-on-write faults. */
static void fork_cow(long n)
{
	size_t size = 16 << 20;
	char *mem = malloc(size);
	memset(mem, 1, size);
	double start = now_us();
	for (long i = 0; i < n; i++) {
		pid_t pid = fork();
		if (pid == 0) {
			for (size_t off = 0; off < size; off += 64 * 4096)
				mem[off] = 2;
			_exit(0);
		}
		waitpid(pid, NULL, 0);
	}
	report("fork_cow", n, start);
	free(mem);
}

/* Map, touch every 4 KiB page, unmap: one page fault per page, and a page
 * table change for each map and unmap. */
static void page_faults(long rounds)
{
	size_t size = 64 << 20;
	double start = now_us();
	for (long i = 0; i < rounds; i++) {
		char *mem = mmap(NULL, size, PROT_READ | PROT_WRITE,
				 MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE, -1, 0);
		if (mem == MAP_FAILED) {
			perror("mmap");
			return;
		}
		for (size_t off = 0; off < size; off += 4096)
			mem[off] = 1;
		munmap(mem, size);
	}
	report("page_fault", rounds * (long)(size / 4096), start);
}

/* Two processes pass a byte back and forth: two process switches, and so
 * two CR3 loads, per round trip. */
static void ping_pong(long n)
{
	int a[2], b[2];
	char c = 0;
	if (pipe(a) || pipe(b)) {
		perror("pipe");
		return;
	}
	pid_t pid = fork();
	if (pid == 0) {
		for (long i = 0; i < n; i++) {
			if (read(a[0], &c, 1) != 1 || write(b[1], &c, 1) != 1)
				_exit(1);
		}
		_exit(0);
	}
	double start = now_us();
	for (long i = 0; i < n; i++) {
		if (write(a[1], &c, 1) != 1 || read(b[0], &c, 1) != 1)
			break;
	}
	report("ctx_switch", n, start);
	waitpid(pid, NULL, 0);
}

int main(int argc, char **argv)
{
	if (argc > 1 && !strcmp(argv[1], "exit"))
		return 0;

	printf("bench: start\n");
	fork_exec(2000);
	fork_cow(1000);
	page_faults(20);
	ping_pong(100000);
	printf("bench: done\n");
	fflush(stdout);
	reboot(RB_AUTOBOOT);	/* no ACPI power off: Hearth stops on the reset */
	return 0;
}
