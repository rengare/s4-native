#!/usr/bin/env bash
# Two native instances on a private virtual LAN (user + network namespaces, no root needed):
# A hosts a game, B opens the LAN lobby and searches. Run inside tools/headless.sh:
#   tools/headless.sh tools/lan_test.sh [outdir]
set -u
root="$(cd "$(dirname "$0")/.." && pwd)"; out="$(realpath -m "${1:-$root/logs/lan}")"; mkdir -p "$out"
bin="$root/target/$(rustc -vV | sed -n 's/host: //p')/release/s4play"
unshare -rn bash -s "$bin" "$root" "$out" <<'INNER'
bin=$1; root=$2; out=$3
ip link set lo up
ip link add v0 type veth peer name v1
unshare -n sleep 600 & NB=$!; sleep 0.5
ip link set v1 netns $NB
ip addr add 10.0.0.1/24 dev v0; ip link set v0 up; ip route add default dev v0
nsenter -t $NB -n bash -c 'ip link set lo up; ip addr add 10.0.0.2/24 dev v1; ip link set v1 up; ip route add default dev v1'
export S4_NO_AUDIO=1 S4_HOST_TRACE=${S4_HOST_TRACE:-}
# Game coordinates: Multiplayer, LAN, Create, next (host); Multiplayer, LAN, Search host (guest).
S4_AUTOCLICK="${HOST_CLICKS:-631,254@8;571,174@14;609,547@20;693,536@28;400,300@60:m}" "$bin" "$root/extracted/s4" >"$out/A.log" 2>&1 &
sleep 2
S4_AUTOCLICK="${GUEST_CLICKS:-631,254@8;571,174@14;368,158@40;400,500@70:m}" nsenter -t $NB -n "$bin" "$root/extracted/s4" >"$out/B.log" 2>&1 &
sleep 52; grim "$out/lan_1.png"; sleep 15; grim "$out/lan_2.png"; sleep ${EXTRA_WAIT:-0}; [ "${EXTRA_WAIT:-0}" != 0 ] && grim "$out/lan_3.png"
for n in $(seq ${EXTRA_SHOTS:-0}); do sleep ${SHOT_EVERY:-10}; grim "$out/s_$n.png"; done
kill $NB; pkill -P $$ -f s4play 2>/dev/null; kill %2 %3 2>/dev/null
INNER
