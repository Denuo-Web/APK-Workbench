#!/usr/bin/env bash

APKW_SUPPORTED_JAVA_MAJORS="${APKW_SUPPORTED_JAVA_MAJORS:-21 17}"

apkw_first_supported_java_major() {
  local major

  for major in $APKW_SUPPORTED_JAVA_MAJORS; do
    printf '%s\n' "$major"
    return 0
  done

  echo "ERROR: APKW_SUPPORTED_JAVA_MAJORS must list at least one Java major version." >&2
  return 1
}

apkw_supported_java_requirement_text() {
  local text=""
  local major

  for major in $APKW_SUPPORTED_JAVA_MAJORS; do
    if [ -n "$text" ]; then
      text="${text} or "
    fi
    text="${text}${major}"
  done

  printf '%s\n' "$text"
}

apkw_supported_java_jre_recommends() {
  local packages=""
  local major

  for major in $APKW_SUPPORTED_JAVA_MAJORS; do
    if [ -n "$packages" ]; then
      packages="${packages} | "
    fi
    packages="${packages}openjdk-${major}-jre"
  done

  printf '%s\n' "$packages"
}

apkw_supported_java_jdk_example() {
  local major

  major="$(apkw_first_supported_java_major)"
  printf 'openjdk-%s-jdk\n' "$major"
}

apkw_java_major_version() {
  local java_bin="$1"
  local version_line

  version_line=$("$java_bin" -version 2>&1 | head -n 1)
  version_line=${version_line#*\"}
  version_line=${version_line%%\"*}
  printf '%s' "${version_line%%.*}"
}

apkw_is_supported_java_major() {
  local major="$1"
  local supported

  for supported in $APKW_SUPPORTED_JAVA_MAJORS; do
    if [ "$supported" = "$major" ]; then
      return 0
    fi
  done

  return 1
}

apkw_find_supported_java() {
  local desired
  local candidate

  for desired in $APKW_SUPPORTED_JAVA_MAJORS; do
    for candidate in /usr/lib/jvm/*; do
      if [ -x "$candidate/bin/java" ] && [ "$(apkw_java_major_version "$candidate/bin/java")" = "$desired" ]; then
        printf '%s\n' "$candidate"
        return 0
      fi
    done
  done

  return 1
}

apkw_is_valid_sdk() {
  local sdk="$1"

  if [ -x "$sdk/platform-tools/adb" ] || [ -x "$sdk/platform-tools/adb.exe" ]; then
    return 0
  fi

  return 1
}

apkw_is_valid_ndk() {
  local ndk="$1"

  if [ -f "$ndk/source.properties" ] && [ -d "$ndk/toolchains/llvm" ]; then
    return 0
  fi

  return 1
}

apkw_list_dirs_by_mtime() {
  local base="$1"

  if [ ! -d "$base" ]; then
    return 0
  fi

  find "$base" -mindepth 1 -maxdepth 1 -type d -printf '%T@ %p\n' 2>/dev/null \
    | sort -nr \
    | cut -d' ' -f2-
}

apkw_pick_latest_valid_sdk() {
  local base="$1"
  local dir

  while IFS= read -r dir; do
    if apkw_is_valid_sdk "$dir"; then
      printf '%s' "$dir"
      return 0
    fi
  done < <(apkw_list_dirs_by_mtime "$base")

  return 1
}

apkw_pick_latest_valid_ndk() {
  local base="$1"
  local dir

  while IFS= read -r dir; do
    if apkw_is_valid_ndk "$dir"; then
      printf '%s' "$dir"
      return 0
    fi
  done < <(apkw_list_dirs_by_mtime "$base")

  return 1
}

apkw_detect_host_page_size() {
  local value="${APKW_HOST_PAGE_SIZE:-}"

  if [[ "$value" =~ ^[0-9]+$ ]] && [ "$value" -gt 0 ]; then
    printf '%s\n' "$value"
    return 0
  fi

  if command -v getconf >/dev/null 2>&1; then
    value="$(getconf PAGESIZE 2>/dev/null || true)"
    if [[ "$value" =~ ^[0-9]+$ ]] && [ "$value" -gt 0 ]; then
      printf '%s\n' "$value"
      return 0
    fi
  fi

  return 1
}

apkw_host_page_profile() {
  local size="${1:-}"

  if [ -z "$size" ]; then
    size="$(apkw_detect_host_page_size || true)"
  fi

  if [[ "$size" =~ ^[0-9]+$ ]] && [ "$size" -gt 4096 ]; then
    printf '16k\n'
  else
    printf '4k\n'
  fi
}

apkw_read_os_release_value() {
  local key="$1"
  local line

  [ -r /etc/os-release ] || return 1

  while IFS= read -r line; do
    case "$line" in
      "${key}="*)
        line="${line#*=}"
        line="${line#\"}"
        line="${line%\"}"
        printf '%s\n' "$line"
        return 0
        ;;
    esac
  done </etc/os-release

  return 1
}

apkw_export_host_profile() {
  local page_size
  local page_profile
  local page_source="${APKW_HOST_PAGE_SIZE_SOURCE:-}"
  local os_id
  local os_version_id
  local os_pretty_name

  if [[ "${APKW_HOST_PAGE_SIZE:-}" =~ ^[0-9]+$ ]] && [ "${APKW_HOST_PAGE_SIZE}" -gt 0 ]; then
    page_size="${APKW_HOST_PAGE_SIZE}"
    if [ -z "$page_source" ]; then
      page_source="env-override"
    fi
  else
    page_size="$(getconf PAGESIZE 2>/dev/null || true)"
    if [[ "$page_size" =~ ^[0-9]+$ ]] && [ "$page_size" -gt 0 ]; then
      page_source="getconf"
    else
      page_size=""
      page_source=""
    fi
  fi

  if [ -n "$page_size" ]; then
    export APKW_HOST_PAGE_SIZE="$page_size"
  fi
  if [ -n "$page_source" ]; then
    export APKW_HOST_PAGE_SIZE_SOURCE="$page_source"
  fi

  page_profile="$(apkw_host_page_profile "$page_size")"
  export APKW_HOST_PAGE_PROFILE="$page_profile"

  os_id="$(apkw_read_os_release_value ID || true)"
  os_version_id="$(apkw_read_os_release_value VERSION_ID || true)"
  os_pretty_name="$(apkw_read_os_release_value PRETTY_NAME || true)"

  [ -n "$os_id" ] && export APKW_HOST_OS_ID="$os_id"
  [ -n "$os_version_id" ] && export APKW_HOST_OS_VERSION_ID="$os_version_id"
  [ -n "$os_pretty_name" ] && export APKW_HOST_OS_PRETTY_NAME="$os_pretty_name"
}

apkw_pick_latest_aapt2() {
  local sdk_root="$1"
  local dir
  local host_arch
  local file_desc

  [ -n "$sdk_root" ] || return 1

  host_arch="$(uname -m)"

  while IFS= read -r dir; do
    if [ -x "$dir/aapt2" ]; then
      file_desc="$(file -b "$dir/aapt2" 2>/dev/null || true)"

      case "$host_arch" in
        aarch64|arm64)
          [[ "$file_desc" == *"ARM aarch64"* ]] || continue
          ;;
        x86_64|amd64)
          [[ "$file_desc" == *"x86-64"* ]] || continue
          ;;
      esac

      printf '%s' "$dir/aapt2"
      return 0
    fi
  done < <(
    find "$sdk_root/build-tools" -mindepth 1 -maxdepth 1 -type d -printf '%f %p\n' 2>/dev/null \
      | sort -Vr \
      | cut -d' ' -f2-
  )

  return 1
}

apkw_sync_sdk_platforms_from_system() {
  local system_sdk_root="${1:-${APKW_SYSTEM_SDK_ROOT:-$HOME/Android/Sdk}}"
  local source_platform
  local platform_name
  local target_platform

  [ -n "${ANDROID_SDK_ROOT:-}" ] || return 0
  [ -d "$system_sdk_root/platforms" ] || return 0
  [ "$system_sdk_root" != "$ANDROID_SDK_ROOT" ] || return 0

  mkdir -p "$ANDROID_SDK_ROOT/platforms"

  for source_platform in "$system_sdk_root"/platforms/android-*; do
    [ -d "$source_platform" ] || continue

    platform_name="$(basename "$source_platform")"
    target_platform="$ANDROID_SDK_ROOT/platforms/$platform_name"

    if [ ! -e "$target_platform" ]; then
      ln -s "$source_platform" "$target_platform"
    fi
  done
}

apkw_prepare_launch_env() {
  local base
  local candidate
  local java_major
  local javac_path
  local ndk_path
  local sdk_path
  local supported_java

  apkw_export_host_profile

  if [ -z "${ANDROID_SDK_ROOT:-}" ]; then
    for base in \
      "$HOME/.local/share/apkw/toolchains/android-sdk-custom" \
      "$HOME/Android/Sdk" \
      "$HOME/Android/sdk"; do
      if sdk_path=$(apkw_pick_latest_valid_sdk "$base"); then
        export ANDROID_SDK_ROOT="$sdk_path"
        export ANDROID_HOME="$sdk_path"
        break
      fi
    done
  fi

  if [ -z "${ANDROID_NDK_ROOT:-}" ]; then
    for base in \
      "$HOME/.local/share/apkw/toolchains/android-ndk-custom" \
      "${ANDROID_SDK_ROOT:-}/ndk" \
      "${ANDROID_SDK_ROOT:-}/ndk-bundle"; do
      if ndk_path=$(apkw_pick_latest_valid_ndk "$base"); then
        export ANDROID_NDK_ROOT="$ndk_path"
        export ANDROID_NDK_HOME="$ndk_path"
        break
      fi
    done
  fi

  if [ -n "${APKW_JAVA_HOME:-}" ]; then
    export JAVA_HOME="$APKW_JAVA_HOME"
  fi

  if [ -z "${JAVA_HOME:-}" ]; then
    if supported_java=$(apkw_find_supported_java); then
      export JAVA_HOME="$supported_java"
    elif command -v javac >/dev/null 2>&1; then
      javac_path=$(readlink -f "$(command -v javac)")
      JAVA_HOME="$(dirname "$(dirname "$javac_path")")"
      export JAVA_HOME
    else
      for candidate in /usr/lib/jvm/*; do
        if [ -x "$candidate/bin/java" ]; then
          export JAVA_HOME="$candidate"
          break
        fi
      done
    fi
  fi

  if [ -n "${JAVA_HOME:-}" ] && [ -x "$JAVA_HOME/bin/java" ]; then
    java_major=$(apkw_java_major_version "$JAVA_HOME/bin/java")
    if ! apkw_is_supported_java_major "$java_major"; then
      if supported_java=$(apkw_find_supported_java); then
        echo "WARN: JAVA_HOME points to Java $java_major; switching to $supported_java for AGP 8.x ($(apkw_supported_java_requirement_text))."
        export JAVA_HOME="$supported_java"
      else
        echo "WARN: JAVA_HOME points to Java $java_major, but AGP 8.x expects Java $(apkw_supported_java_requirement_text)."
      fi
    fi
  fi

  if [ -n "${JAVA_HOME:-}" ] && [[ ":$PATH:" != *":$JAVA_HOME/bin:"* ]]; then
    export PATH="$JAVA_HOME/bin:$PATH"
  fi

  if [ -n "${ANDROID_SDK_ROOT:-}" ] && [ -d "$ANDROID_SDK_ROOT/platform-tools" ] && [[ ":$PATH:" != *":$ANDROID_SDK_ROOT/platform-tools:"* ]]; then
    export PATH="$ANDROID_SDK_ROOT/platform-tools:$PATH"
  fi

  if [ -z "${APKW_ADB_PATH:-}" ] && [ -n "${ANDROID_SDK_ROOT:-}" ]; then
    if [ -x "$ANDROID_SDK_ROOT/platform-tools/adb" ]; then
      export APKW_ADB_PATH="$ANDROID_SDK_ROOT/platform-tools/adb"
    elif [ -x "$ANDROID_SDK_ROOT/platform-tools/adb.exe" ]; then
      export APKW_ADB_PATH="$ANDROID_SDK_ROOT/platform-tools/adb.exe"
    fi
  fi
}

apkw_print_launch_env_summary() {
  local example_jdk

  echo "Environment:"
  echo "  APKW_HOST_PAGE_SIZE=${APKW_HOST_PAGE_SIZE:-<unset>}"
  echo "  APKW_HOST_PAGE_SIZE_SOURCE=${APKW_HOST_PAGE_SIZE_SOURCE:-<unset>}"
  echo "  APKW_HOST_PAGE_PROFILE=${APKW_HOST_PAGE_PROFILE:-<unset>}"
  echo "  APKW_HOST_OS_ID=${APKW_HOST_OS_ID:-<unset>}"
  echo "  APKW_HOST_OS_VERSION_ID=${APKW_HOST_OS_VERSION_ID:-<unset>}"
  echo "  ANDROID_SDK_ROOT=${ANDROID_SDK_ROOT:-<unset>}"
  echo "  ANDROID_NDK_ROOT=${ANDROID_NDK_ROOT:-<unset>}"
  echo "  JAVA_HOME=${JAVA_HOME:-<unset>}"
  echo "  APKW_ADB_PATH=${APKW_ADB_PATH:-<unset>}"
  echo

  if [ -z "${ANDROID_SDK_ROOT:-}" ]; then
    echo "WARN: ANDROID_SDK_ROOT not set. Install the SDK via Toolchains or set ANDROID_SDK_ROOT."
  elif ! apkw_is_valid_sdk "$ANDROID_SDK_ROOT"; then
    echo "WARN: ANDROID_SDK_ROOT does not look like a full SDK (missing platform-tools/adb)."
  fi

  if [ -z "${JAVA_HOME:-}" ]; then
    example_jdk="$(apkw_supported_java_jdk_example)"
    echo "WARN: JAVA_HOME not set. Install a JDK (e.g. $example_jdk)."
  fi

  echo
}
