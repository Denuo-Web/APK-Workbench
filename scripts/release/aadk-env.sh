#!/usr/bin/env bash

AADK_SUPPORTED_JAVA_MAJORS="${AADK_SUPPORTED_JAVA_MAJORS:-21 17}"

aadk_first_supported_java_major() {
  local major

  for major in $AADK_SUPPORTED_JAVA_MAJORS; do
    printf '%s\n' "$major"
    return 0
  done

  echo "ERROR: AADK_SUPPORTED_JAVA_MAJORS must list at least one Java major version." >&2
  return 1
}

aadk_supported_java_requirement_text() {
  local text=""
  local major

  for major in $AADK_SUPPORTED_JAVA_MAJORS; do
    if [ -n "$text" ]; then
      text="${text} or "
    fi
    text="${text}${major}"
  done

  printf '%s\n' "$text"
}

aadk_supported_java_jre_recommends() {
  local packages=""
  local major

  for major in $AADK_SUPPORTED_JAVA_MAJORS; do
    if [ -n "$packages" ]; then
      packages="${packages} | "
    fi
    packages="${packages}openjdk-${major}-jre"
  done

  printf '%s\n' "$packages"
}

aadk_supported_java_jdk_example() {
  local major

  major="$(aadk_first_supported_java_major)"
  printf 'openjdk-%s-jdk\n' "$major"
}

aadk_java_major_version() {
  local java_bin="$1"
  local version_line

  version_line=$("$java_bin" -version 2>&1 | head -n 1)
  version_line=${version_line#*\"}
  version_line=${version_line%%\"*}
  printf '%s' "${version_line%%.*}"
}

aadk_is_supported_java_major() {
  local major="$1"
  local supported

  for supported in $AADK_SUPPORTED_JAVA_MAJORS; do
    if [ "$supported" = "$major" ]; then
      return 0
    fi
  done

  return 1
}

aadk_find_supported_java() {
  local desired
  local candidate

  for desired in $AADK_SUPPORTED_JAVA_MAJORS; do
    for candidate in /usr/lib/jvm/*; do
      if [ -x "$candidate/bin/java" ] && [ "$(aadk_java_major_version "$candidate/bin/java")" = "$desired" ]; then
        printf '%s\n' "$candidate"
        return 0
      fi
    done
  done

  return 1
}

aadk_is_valid_sdk() {
  local sdk="$1"

  if [ -x "$sdk/platform-tools/adb" ] || [ -x "$sdk/platform-tools/adb.exe" ]; then
    return 0
  fi

  return 1
}

aadk_is_valid_ndk() {
  local ndk="$1"

  if [ -f "$ndk/source.properties" ] && [ -d "$ndk/toolchains/llvm" ]; then
    return 0
  fi

  return 1
}

aadk_list_dirs_by_mtime() {
  local base="$1"

  if [ ! -d "$base" ]; then
    return 0
  fi

  find "$base" -mindepth 1 -maxdepth 1 -type d -printf '%T@ %p\n' 2>/dev/null \
    | sort -nr \
    | cut -d' ' -f2-
}

aadk_pick_latest_valid_sdk() {
  local base="$1"
  local dir

  while IFS= read -r dir; do
    if aadk_is_valid_sdk "$dir"; then
      printf '%s' "$dir"
      return 0
    fi
  done < <(aadk_list_dirs_by_mtime "$base")

  return 1
}

aadk_pick_latest_valid_ndk() {
  local base="$1"
  local dir

  while IFS= read -r dir; do
    if aadk_is_valid_ndk "$dir"; then
      printf '%s' "$dir"
      return 0
    fi
  done < <(aadk_list_dirs_by_mtime "$base")

  return 1
}

aadk_prepare_launch_env() {
  local base
  local candidate
  local java_major
  local javac_path
  local ndk_path
  local sdk_path
  local supported_java

  if [ -z "${ANDROID_SDK_ROOT:-}" ]; then
    for base in "$HOME/.local/share/aadk/toolchains/android-sdk-custom" "$HOME/Android/Sdk" "$HOME/Android/sdk"; do
      if sdk_path=$(aadk_pick_latest_valid_sdk "$base"); then
        export ANDROID_SDK_ROOT="$sdk_path"
        export ANDROID_HOME="$sdk_path"
        break
      fi
    done
  fi

  if [ -z "${ANDROID_NDK_ROOT:-}" ]; then
    for base in "$HOME/.local/share/aadk/toolchains/android-ndk-custom" "${ANDROID_SDK_ROOT:-}/ndk" "${ANDROID_SDK_ROOT:-}/ndk-bundle"; do
      if ndk_path=$(aadk_pick_latest_valid_ndk "$base"); then
        export ANDROID_NDK_ROOT="$ndk_path"
        export ANDROID_NDK_HOME="$ndk_path"
        break
      fi
    done
  fi

  if [ -n "${AADK_JAVA_HOME:-}" ]; then
    export JAVA_HOME="$AADK_JAVA_HOME"
  fi

  if [ -z "${JAVA_HOME:-}" ]; then
    if supported_java=$(aadk_find_supported_java); then
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
    java_major=$(aadk_java_major_version "$JAVA_HOME/bin/java")
    if ! aadk_is_supported_java_major "$java_major"; then
      if supported_java=$(aadk_find_supported_java); then
        echo "WARN: JAVA_HOME points to Java $java_major; switching to $supported_java for AGP 8.x ($(aadk_supported_java_requirement_text))."
        export JAVA_HOME="$supported_java"
      else
        echo "WARN: JAVA_HOME points to Java $java_major, but AGP 8.x expects Java $(aadk_supported_java_requirement_text)."
      fi
    fi
  fi

  if [ -n "${JAVA_HOME:-}" ] && [[ ":$PATH:" != *":$JAVA_HOME/bin:"* ]]; then
    export PATH="$JAVA_HOME/bin:$PATH"
  fi

  if [ -n "${ANDROID_SDK_ROOT:-}" ] && [ -d "$ANDROID_SDK_ROOT/platform-tools" ] && [[ ":$PATH:" != *":$ANDROID_SDK_ROOT/platform-tools:"* ]]; then
    export PATH="$ANDROID_SDK_ROOT/platform-tools:$PATH"
  fi

  if [ -z "${AADK_ADB_PATH:-}" ] && [ -n "${ANDROID_SDK_ROOT:-}" ]; then
    if [ -x "$ANDROID_SDK_ROOT/platform-tools/adb" ]; then
      export AADK_ADB_PATH="$ANDROID_SDK_ROOT/platform-tools/adb"
    elif [ -x "$ANDROID_SDK_ROOT/platform-tools/adb.exe" ]; then
      export AADK_ADB_PATH="$ANDROID_SDK_ROOT/platform-tools/adb.exe"
    fi
  fi
}

aadk_print_launch_env_summary() {
  local example_jdk

  echo "Environment:"
  echo "  ANDROID_SDK_ROOT=${ANDROID_SDK_ROOT:-<unset>}"
  echo "  ANDROID_NDK_ROOT=${ANDROID_NDK_ROOT:-<unset>}"
  echo "  JAVA_HOME=${JAVA_HOME:-<unset>}"
  echo "  AADK_ADB_PATH=${AADK_ADB_PATH:-<unset>}"
  echo

  if [ -z "${ANDROID_SDK_ROOT:-}" ]; then
    echo "WARN: ANDROID_SDK_ROOT not set. Install the SDK via Toolchains or set ANDROID_SDK_ROOT."
  elif ! aadk_is_valid_sdk "$ANDROID_SDK_ROOT"; then
    echo "WARN: ANDROID_SDK_ROOT does not look like a full SDK (missing platform-tools/adb)."
  fi

  if [ -z "${JAVA_HOME:-}" ]; then
    example_jdk="$(aadk_supported_java_jdk_example)"
    echo "WARN: JAVA_HOME not set. Install a JDK (e.g. $example_jdk)."
  fi

  echo
}
