#!/bin/sh
# Arguments are passed directly by Rust; paths never become shell source.
pid=$1
bundle=$2
stage=$3
log=$4
original_stamp=$5
shift 5
launch_app() {
	/usr/bin/open -n "$bundle" "$@"
}
backup="$stage/old.app"
new="$stage/new.app"
# This helper remains outside both app bundles and survives their replacement.
trap '' HUP
report() {
	message="$(/bin/cat "$stage/message-$1") ${2:-} $(/bin/cat "$stage/message-log") $log"
	title=$(/bin/cat "$stage/message-title")
	printf '%s\n' "$message"
	# Pass the message as argv, rather than interpolating it into AppleScript.
	/usr/bin/osascript - "$title" "$message" <<'APPLESCRIPT'
on run argv
	display alert (item 1 of argv) message (item 2 of argv) as warning
end run
APPLESCRIPT
}
cleanup() {
	/bin/rm -rf "$stage"
}
printf '%s\n' "$$" > "$stage/helper-pid" || exit 1
: > "$stage/ready" || exit 1
# Cancelled UI flows drop the staging directory. Never touch the app without commit.
count=0
while [ ! -f "$stage/commit" ]; do
	if [ ! -d "$stage" ]; then
		/bin/rm -f "$log"
		exit 0
	fi
	count=$((count + 1))
	if [ "$count" -ge 300 ]; then
		printf '%s\n' 'Installation was not committed; keeping the current app.'
		cleanup
		exit 1
	fi
	/bin/sleep 0.2
done
count=0
while /bin/kill -0 "$pid" 2>/dev/null; do
	count=$((count + 1))
	if [ "$count" -ge 300 ]; then
		report 0
		cleanup
		exit 1
	fi
	/bin/sleep 0.2
done
printf '%s\n' 'Application exited; replacing the bundle.'
rename_bundle() {
	# rename(2) fails on an occupied, nonempty destination instead of nesting the source
	# inside it. /usr/bin/perl ships with our supported macOS versions.
	/usr/bin/perl -e 'rename $ARGV[0], $ARGV[1] or die "rename: $!\n"' "$1" "$2"
}
# Recheck after waiting: another installer may have changed the destination.
if [ ! -d "$bundle" ] || [ -L "$bundle" ] || [ ! -d "$new" ] || [ "$(/usr/bin/stat -f '%d:%i' "$bundle")" != "$original_stamp" ]; then
	report 1
	cleanup
	exit 1
fi
if ! rename_bundle "$bundle" "$backup"; then
	report 2
	launch_app "$@"
	cleanup
	exit 1
fi
if ! rename_bundle "$new" "$bundle"; then
	if rename_bundle "$backup" "$bundle"; then
		report 3
		launch_app "$@"
		cleanup
	else
		report 4 "$backup"
	fi
	exit 1
fi
if ! launch_app "$@"; then
	report 5 "$backup"
	exit 1
fi
printf '%s\n' 'Replacement completed and macOS accepted the launch request.'
cleanup
/bin/rm -f "$log"
