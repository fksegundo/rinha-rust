#!/usr/bin/env bash
# perf_capture.sh — non-invasive profiling of the rinha-rust stack
#
# Prerequisites:
#   - Stack running with perf-enabled images (make up-perf)
#   - sudo access for perf, strace, bpftrace
#   - inferno-flamegraph (cargo install inferno) or FlameGraph tools
#
# Usage:
#   ./scripts/perf_capture.sh [output_dir] [duration_secs]
#
# Output:
#   output_dir/
#     flame-api1.svg, flame-api2.svg, flame-lb.svg
#     strace-api1.txt, strace-api2.txt, strace-lb.txt
#     bpftrace-read-write.txt
#     bpftrace-handoff.txt
#     cpu-throttle.txt
#     summary.txt

set -euo pipefail

OUTPUT_DIR="${1:-test/perf-capture}"
DURATION="${2:-30}"
COMPOSE_PROJECT="${COMPOSE_PROJECT:-submission}"

mkdir -p "$OUTPUT_DIR"

# ── Resolve container PIDs ────────────────────────────────────────────────────

API1_PID=$(docker inspect -f '{{.State.Pid}}' "${COMPOSE_PROJECT}-api1-1" 2>/dev/null || echo "")
API2_PID=$(docker inspect -f '{{.State.Pid}}' "${COMPOSE_PROJECT}-api2-1" 2>/dev/null || echo "")
LB_PID=$(docker inspect -f '{{.State.Pid}}' "${COMPOSE_PROJECT}-lb-1" 2>/dev/null || echo "")

if [ -z "$API1_PID" ] || [ -z "$LB_PID" ]; then
    echo "ERROR: could not resolve container PIDs. Is the stack running?"
    echo "  API1_PID=$API1_PID  API2_PID=$API2_PID  LB_PID=$LB_PID"
    exit 1
fi

echo "=== perf_capture.sh ==="
echo "API1 PID: $API1_PID"
echo "API2 PID: $API2_PID"
echo "LB PID:   $LB_PID"
echo "Duration: ${DURATION}s"
echo "Output:   $OUTPUT_DIR"
echo ""

# ── 1. perf record + flamegraphs ──────────────────────────────────────────────

echo "[1/5] perf record (${DURATION}s each) ..."

for name in api1 api2 lb; do
    pid_var="${name^^}_PID"  # API1_PID, API2_PID, LB_PID
    pid="${!pid_var}"
    if [ -z "$pid" ]; then
        echo "  skipping $name (no PID)"
        continue
    fi
    data_file="$OUTPUT_DIR/perf-${name}.data"
    svg_file="$OUTPUT_DIR/flame-${name}.svg"

    sudo perf record -F 999 -p "$pid" -g --call-graph dwarf -o "$data_file" -- sleep "$DURATION" 2>/dev/null || {
        echo "  WARNING: perf record failed for $name (pid=$pid). Try:"
        echo "    sudo sh -c 'echo 0 > /proc/sys/kernel/perf_event_paranoid'"
        echo "    sudo sh -c 'echo kernel.kptr_restrict=0 >> /etc/sysctl.conf'"
        continue
    }

    # Generate flamegraph
    if command -v inferno-flamegraph &>/dev/null; then
        sudo perf script -i "$data_file" 2>/dev/null | \
            inferno-collapse-perf 2>/dev/null | \
            inferno-flamegraph > "$svg_file" 2>/dev/null && \
            echo "  $name: $svg_file" || \
            echo "  $name: perf data saved, flamegraph generation failed"
    else
        echo "  $name: $data_file saved (install inferno for SVG: cargo install inferno)"
    fi
done

# ── 2. strace syscall summary ─────────────────────────────────────────────────

echo "[2/5] strace syscall summary (${DURATION}s) ..."

for name in api1 api2 lb; do
    pid_var="${name^^}_PID"
    pid="${!pid_var}"
    if [ -z "$pid" ]; then
        continue
    fi
    out="$OUTPUT_DIR/strace-${name}.txt"
    sudo timeout "$DURATION" strace -c -f -p "$pid" -o "$out" 2>/dev/null &
done

# ── 3. bpftrace: read/write latency histograms ────────────────────────────────

echo "[3/5] bpftrace read/write histograms (${DURATION}s) ..."

bpftrace_script="$OUTPUT_DIR/bpftrace-read-write.bt"
cat > "$bpftrace_script" <<'BTEOF'
BEGIN { printf("tracing read/write for %ds...\n", $1); }

tracepoint:syscalls:sys_enter_read  /pid == $2 || pid == $3/ { @rs[tid] = nsecs; }
tracepoint:syscalls:sys_exit_read   /pid == $2 || pid == $3 && @rs[tid]/ {
    @read_us = hist((nsecs - @rs[tid]) / 1000); delete(@rs[tid]);
}
tracepoint:syscalls:sys_enter_write /pid == $2 || pid == $3/ { @ws[tid] = nsecs; }
tracepoint:syscalls:sys_exit_write  /pid == $2 || pid == $3 && @ws[tid]/ {
    @write_us = hist((nsecs - @ws[tid]) / 1000); delete(@ws[tid]);
}

interval:s:$1 { exit(); }
BTEOF

sudo bpftrace "$bpftrace_script" "$DURATION" "$API1_PID" "$API2_PID" \
    > "$OUTPUT_DIR/bpftrace-read-write.txt" 2>/dev/null &
BPF_RW_PID=$!

# ── 4. bpftrace: handoff latency (LB sendmsg → API recvmsg) ───────────────────

echo "[4/5] bpftrace handoff latency (${DURATION}s) ..."

# We use kprobe on sys_sendmsg / sys_recvmsg filtered by PID
bpftrace_handoff="$OUTPUT_DIR/bpftrace-handoff.bt"
cat > "$bpftrace_handoff" <<'BTEOF'
BEGIN { printf("tracing handoff for %ds...\n", $1); }

kprobe:__sys_sendmsg /pid == $2/ {
    @send[tid] = nsecs;
}
kprobe:__sys_recvmsg /pid == $3 || pid == $4/ {
    @recv[tid] = nsecs;
}

// Match send→recv pairs within a short window
// This is approximate — uprobe on the actual functions is more precise
// but requires the perf-enabled binary with symbols.

interval:s:$1 { exit(); }

END {
    printf("sendmsg calls: %d\n", count(@send));
    printf("recvmsg calls: %d\n", count(@recv));
}
BTEOF

sudo bpftrace "$bpftrace_handoff" "$DURATION" "$LB_PID" "$API1_PID" "$API2_PID" \
    > "$OUTPUT_DIR/bpftrace-handoff.txt" 2>/dev/null &
BPF_HO_PID=$!

# ── 5. CFS throttle stats ─────────────────────────────────────────────────────

echo "[5/5] CFS throttle stats ..."

for name in api1 api2 lb; do
    cid="${COMPOSE_PROJECT}-${name}-1"
    {
        echo "=== $name ==="
        docker exec "$cid" cat /sys/fs/cgroup/cpu.stat 2>/dev/null || \
            echo "(cpu.stat not available — cgroup v1?)"
        echo ""
    } >> "$OUTPUT_DIR/cpu-throttle.txt"
done

# ── Wait for background jobs ──────────────────────────────────────────────────

echo ""
echo "Waiting for background captures to finish (${DURATION}s + buffer) ..."
wait $BPF_RW_PID 2>/dev/null || true
wait $BPF_HO_PID 2>/dev/null || true
wait  # strace children

# ── Summary ────────────────────────────────────────────────────────────────────

{
    echo "=== PERF CAPTURE SUMMARY ==="
    echo "Date: $(date -Iseconds)"
    echo "Duration: ${DURATION}s"
    echo ""
    echo "PIDs: api1=$API1_PID api2=$API2_PID lb=$LB_PID"
    echo ""
    echo "Files:"
    ls -lh "$OUTPUT_DIR/" 2>/dev/null
    echo ""
    echo "=== CPU THROTTLE ==="
    cat "$OUTPUT_DIR/cpu-throttle.txt" 2>/dev/null || echo "(none)"
    echo ""
    echo "=== STRACE TOP SYSCALLS (api1) ==="
    head -50 "$OUTPUT_DIR/strace-api1.txt" 2>/dev/null || echo "(none)"
} > "$OUTPUT_DIR/summary.txt"

echo ""
echo "Done. Results in $OUTPUT_DIR/"
echo "  summary:  $OUTPUT_DIR/summary.txt"
echo "  flames:   $OUTPUT_DIR/flame-*.svg"
echo "  strace:   $OUTPUT_DIR/strace-*.txt"
echo "  bpftrace: $OUTPUT_DIR/bpftrace-*.txt"
echo "  throttle: $OUTPUT_DIR/cpu-throttle.txt"
