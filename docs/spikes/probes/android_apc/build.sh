#!/bin/sh
# Builds apc-spike.apk with plain SDK tools (no Gradle). Needs ANDROID_HOME and a JDK.
set -e
SDK=${ANDROID_HOME:-$HOME/Android/Sdk}
BT=$SDK/build-tools/36.0.0
JAR=$SDK/platforms/android-35/android.jar
OUT=${1:-out}
rm -rf "$OUT"; mkdir -p "$OUT/classes"
"$BT/aapt2" link --manifest AndroidManifest.xml -I "$JAR" -o "$OUT/base.apk" --min-sdk-version 29 --target-sdk-version 35
javac --release 11 -classpath "$JAR" -d "$OUT/classes" src/com/urdevice/spike/*.java
"$BT/d8" --lib "$JAR" --min-api 29 --output "$OUT" "$OUT"/classes/com/urdevice/spike/*.class
cp "$OUT/base.apk" "$OUT/unsigned.apk"
(cd "$OUT" && zip -q unsigned.apk classes.dex)
"$BT/zipalign" -f 4 "$OUT/unsigned.apk" "$OUT/aligned.apk"
"$BT/apksigner" sign --ks "$HOME/.android/debug.keystore" --ks-pass pass:android --out "$OUT/apc-spike.apk" "$OUT/aligned.apk"
echo "built $OUT/apc-spike.apk"
