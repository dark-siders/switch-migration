#!/bin/sh
# distro_smoke.sh — проверка бинаря и рантайм-утилит на разных дистрибутивах.
# Чистый POSIX sh (без bash-конструкций), чтобы работал и на образах без bash
# (voidlinux/voidlinux:latest использует только /bin/sh = dash).
#
# Используется из .gitlab-ci.yml (stage smoke, джобы distro-smoke-*).
# Два аргумента (необязательные):
#   $1 — путь к бинарю (default: ./switch-migrator-x86_64)
#   $2 — инструменты-исключения через пробел (default: пусто)

set -e

BIN="${1:-./switch-migrator-x86_64}"
SKIP="${2:-}"

echo "=== smoke: $BIN ==="

if [ ! -x "$BIN" ]; then
  echo "MISSING binary: $BIN (не найден или не исполняемый)"
  exit 1
fi

"$BIN" ls

# Рантайм-утилиты должны быть на месте (имена пакетов различаются по дистро).
TOOLS="sfdisk wipefs partprobe partx mkfs.vfat mount umount blockdev dd sync which"
PROBLEM=0

for tool in $TOOLS; do
  case " $SKIP " in
    *" $tool "*) continue ;;
  esac
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "MISSING tool: $tool"
    PROBLEM=1
  fi
done

if [ "$PROBLEM" -ne 0 ]; then
  exit 1
fi

echo "=== smoke ok: все рантайм-утилиты на месте ==="