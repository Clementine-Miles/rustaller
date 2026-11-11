#!/usr/bin/env bash
# shellcheck shell=dash
# shellcheck disable=SC3010
# Alpine/OpenRC uses BusyBox ash; systemd targets use Bash.

set -eE

# First boot: resolve final NIC names by MAC after the installed kernel loads.
# Alpine/Debian use ifupdown; Arch/Gentoo use systemd-networkd.
# The installed kernel can expose different NIC names/order than the initrd.

has_eth=false
check_count=0
stable_count=0
old_state=
while true; do
    check_count=$((check_count + 1))

    new_state=$(ip -o link | awk '$2 != "lo:"')
    if [ -n "$new_state" ]; then
        has_eth=true
    fi

    if $has_eth && [ "$old_state" = "$new_state" ]; then
        stable_count=$((stable_count + 1))
    else
        stable_count=0
    fi

    old_state=$new_state
    echo "Waiting for the NICs to be stable (${stable_count}s/10s)..."

    if $has_eth && [ "$stable_count" -ge 10 ]; then
        break
    fi
    if ! $has_eth && [ "$check_count" -ge 60 ]; then
        exit 1
    fi
    sleep 1
done

to_lower() {
    tr '[:upper:]' '[:lower:]'
}

get_ethx_by_mac() {
    mac=$(echo "$1" | to_lower)
    flag=$2
    if [ -z "$flag" ]; then
        flag=master
    fi

    if [ "$flag" = master ]; then
        # Azure accelerated-networking VF has the same MAC but a master field.
        ip -o link | grep -i "$mac" | grep -v master | awk '{print $2}' | cut -d: -f1 | grep .
    else
        ip -o link | grep -i "$mac" | grep -w master | awk '{print $2}' | cut -d: -f1 | grep .
    fi
}

fix_ifupdown() {
    file=/etc/network/interfaces
    tmp_file=$file.tmp

    rm -f "$tmp_file"

    if [ -f "$file" ]; then
        while IFS= read -r line; do
            del_this_line=false
            if [[ "$line" = "# mac "* ]]; then
                ethx=
                if mac=$(echo "$line" | awk '{print $NF}'); then
                    ethx=$(get_ethx_by_mac "$mac") || true
                fi
                del_this_line=true
            elif [[ "$line" = "iface e"* ]] ||
                [[ "$line" = "auto e"* ]] ||
                [[ "$line" = "allow-hotplug e"* ]]; then
                if [ -n "$ethx" ]; then
                    line=$(echo "$line" | awk "{\$2=\"$ethx\"; print \$0}")
                fi
            elif [[ "$line" = *" dev e"* ]]; then
                if [ -n "$ethx" ]; then
                    # Keep indentation; awk would discard the leading spaces.
                    line=$(echo "$line" | sed -E "s/[^ ]*$/$ethx/")
                fi
            fi
            if ! $del_this_line; then
                echo "$line" >>"$tmp_file"
            fi
        done <"$file"

        mv "$tmp_file" "$file"
    fi
}

fix_systemd_networkd() {
    for file in /etc/systemd/network/10-cloud-init-eth*.network; do
        [ -f "$file" ] || continue
        mac=$(grep ^MACAddress= "$file" | cut -d= -f2 | grep .) || continue
        ethx=$(get_ethx_by_mac "$mac") || continue

        proper_file=/etc/systemd/network/10-$ethx.network
        sed -Ei "s/^Name=eth[0-9]+/Name=$ethx/" "$file"
        mv "$file" "$proper_file"
    done
}

fix_ifupdown
fix_systemd_networkd
