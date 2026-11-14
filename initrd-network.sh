#!/bin/ash
# shellcheck shell=dash
# Alpine Live initrd networking helper

# accept_ra take over RA + Automatically configure gateway
# autoconf  Automatically configure the address, depending on accept_ra

mac_addr=$1
ipv4_addr=$2
ipv4_gateway=$3
ipv6_addr=$4
ipv6_gateway=$5
ipv6_extra_addrs=$6

DHCP_TIMEOUT=15
DNS_FILE_TIMEOUT=5
TEST_TIMEOUT=10

# Detecting whether there is a network is by detecting these DNS endpoint Is the port open?
# Change to generate_204？But when detecting the network, it is possible resolv.conf is empty
# HTTP 80
# HTTPS/DOH 443
# DOT 853
ipv4_dns1='1.1.1.1'
ipv4_dns2='8.8.8.8' # Not open 80
ipv6_dns1='2606:4700:4700::1111'
ipv6_dns2='2001:4860:4860::8888' # Not open 80

# Find the primary network card
get_ethx() {
    # filter azure vf (bring master ethx)
    # 2: eth0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc mq state UP qlen 1000\    link/ether 60:45:bd:21:8a:51 brd ff:ff:ff:ff:ff:ff
    # 3: eth1: <BROADCAST,MULTICAST,UP,LOWER_UP800> mtu 1500 qdisc mq master eth0 state UP qlen 1000\    link/ether 60:45:bd:21:8a:51 brd ff:ff:ff
    ip -o link | grep -i "$mac_addr" | grep -v master | cut -d' ' -f2 | cut -d: -f1 | grep .
}

get_ipv4_gateway() {
    ip -4 route show default dev "$ethx" | head -1 | cut -d ' ' -f3
}

get_ipv6_gateway() {
    ip -6 route show default dev "$ethx" | head -1 | cut -d ' ' -f3
}

get_first_ipv4_addr() {
    ip -4 -o addr show scope global dev "$ethx" | head -1 | grep -o '[0-9\.]*/[0-9]*'
}

get_first_ipv4_gateway() {
    ip -4 route show default dev "$ethx" | head -1 | cut -d' ' -f3
}

remove_netmask() {
    cut -d/ -f1
}

get_first_ipv6_addr() {
    ip -6 -o addr show scope global dev "$ethx" | head -1 | grep -o '[0-9a-f\:]*/[0-9]*'
}

get_first_ipv6_gateway() {
    ip -6 route show default dev "$ethx" | head -1 | cut -d' ' -f3
}

is_have_ipv4_addr() {
    ip -4 addr show scope global dev "$ethx" | grep -q inet
}

is_have_ipv6_addr() {
    ip -6 addr show scope global dev "$ethx" | grep -q inet6
}

is_have_ipv4_gateway() {
    ip -4 route show default dev "$ethx" | grep -q .
}

is_have_ipv6_gateway() {
    ip -6 route show default dev "$ethx" | grep -q .
}

is_have_ipv4() {
    is_have_ipv4_addr && is_have_ipv4_gateway
}

is_have_ipv6() {
    is_have_ipv6_addr && is_have_ipv6_gateway
}

is_have_ipv4_dns() {
    [ -f /etc/resolv.conf ] && grep -q '^nameserver .*\.' /etc/resolv.conf
}

is_have_ipv6_dns() {
    [ -f /etc/resolv.conf ] && grep -q '^nameserver .*:' /etc/resolv.conf
}

add_missing_ipv4_config() {
    if [ -n "$ipv4_addr" ] && [ -n "$ipv4_gateway" ]; then
        if ! is_have_ipv4_addr; then
            ip -4 addr add "$ipv4_addr" dev "$ethx"
        fi

        if ! is_have_ipv4_gateway; then
            # if dhcp Unable to setonlinkGateway, then set it here
            if true; then
                ip -4 route add "$ipv4_gateway" dev "$ethx"
                ip -4 route add default via "$ipv4_gateway" dev "$ethx"
            else
                ip -4 route add default via "$ipv4_gateway" dev "$ethx" onlink
            fi
        fi
    fi
}

add_missing_ipv6_config() {
    if [ -n "$ipv6_addr" ] && [ -n "$ipv6_gateway" ]; then
        if ! is_have_ipv6_addr; then
            ip -6 addr add "$ipv6_addr" dev "$ethx"
        fi

        if ! is_have_ipv6_gateway; then
            # if dhcp Unable to setonlinkGateway, then set it here
            if true; then
                ip -6 route add "$ipv6_gateway" dev "$ethx"
                ip -6 route add default via "$ipv6_gateway" dev "$ethx"
            else
                ip -6 route add default via "$ipv6_gateway" dev "$ethx" onlink
            fi
        fi

        # add extra IPv6 Addresses (comma separated）
        # On Tencent Cloud，dhcpv6 obtained IPv6 Not from the old system IPv6，and unavailable
        # Script to add old system IPv6 address and extra IPv6 After the address, additional IPv6 The address is at ip -6 addr first place
        # get_first_ipv6_addr What you get is extra IPv6 Address, that is, unavailable IPv6 address
        # lead to ipv6 test_internet fail
        # Therefore it is temporarily closed ipv6_extra_addrs
    fi
}

is_need_test_ipv4() {
    is_have_ipv4 && ! $ipv4_has_internet
}

is_need_test_ipv6() {
    is_have_ipv6 && ! $ipv6_has_internet
}

# nc detection TCP 443 Is it reachable?

test_connect() {
    src=$1
    dst=$2
    nc -z -v -w "$TEST_TIMEOUT" -s "$src" "$dst" 443
}

test_internet() {
    for i in $(seq 5); do
        echo "Testing Internet Connection. Test $i... "
        if is_need_test_ipv4 &&
            current_ipv4_addr="$(get_first_ipv4_addr | remove_netmask)" &&
            { test_connect "$current_ipv4_addr" "$ipv4_dns1" ||
                test_connect "$current_ipv4_addr" "$ipv4_dns2"; } >/dev/null 2>&1; then
            echo "IPv4 has internet."
            ipv4_has_internet=true
        fi
        if is_need_test_ipv6 &&
            current_ipv6_addr="$(get_first_ipv6_addr | remove_netmask)" &&
            { test_connect "$current_ipv6_addr" "$ipv6_dns1" ||
                test_connect "$current_ipv6_addr" "$ipv6_dns2"; } >/dev/null 2>&1; then
            echo "IPv6 has internet."
            ipv6_has_internet=true
        fi
        if ! is_need_test_ipv4 && ! is_need_test_ipv6; then
            break
        fi
        sleep 1
    done
}

flush_ipv4_config() {
    ip -4 addr flush scope global dev "$ethx"
    ip -4 route flush dev "$ethx"
    # DHCP obtained IP Not before reinstallation IP when, delete them together DHCP obtained DNS，Just in case DNS invalid
    sed -i "/\./d" /etc/resolv.conf
}

should_disable_dhcpv4=false
should_disable_accept_ra=false
should_disable_autoconf=false

flush_ipv6_config() {
    if $should_disable_accept_ra; then
        echo 0 >"/proc/sys/net/ipv6/conf/$ethx/accept_ra"
    fi
    if $should_disable_autoconf; then
        echo 0 >"/proc/sys/net/ipv6/conf/$ethx/autoconf"
    fi
    ip -6 addr flush scope global dev "$ethx"
    ip -6 route flush dev "$ethx"
    # DHCP obtained IP Not before reinstallation IP when, delete them together DHCP obtained DNS，Just in case DNS invalid
    sed -i "/:/d" /etc/resolv.conf
}

for i in $(seq 20); do
    if ethx=$(get_ethx); then
        break
    fi
    sleep 1
done

if [ -z "$ethx" ]; then
    echo "Not found network card: $mac_addr"
    exit
fi

echo "Configuring $ethx ($mac_addr)..."

# Not open lo but frp Unable to connect 127.0.0.1 22
ip link set dev lo up

# turn on ethx
ip link set dev "$ethx" up
sleep 1

# Alpine Live DHCPv4/v6. A timeout bounds providers' noisy udhcpc retries.
# dhcpcd may expire its lease when stopped, so use udhcpc for this one-shot probe.
method=udhcpc
case "$method" in
udhcpc)
    timeout "$DHCP_TIMEOUT" udhcpc -i "$ethx" -f -q -n || true
    timeout "$DHCP_TIMEOUT" udhcpc6 -i "$ethx" -f -q -n || true
    sleep "$DNS_FILE_TIMEOUT"
    ;;
dhcpcd)
    grep -q dhcpcd /etc/group || addgroup -S dhcpcd
    grep -q dhcpcd /etc/passwd || adduser -S -D -H \
        -h /var/lib/dhcpcd \
        -s /sbin/nologin \
        -G dhcpcd \
        -g dhcpcd \
        dhcpcd
    dhcpcd --persistent --noipv4ll "$ethx"
    sleep "$DNS_FILE_TIMEOUT"
    dhcpcd -x "$ethx"
    sysctl -w "net.ipv6.conf.$ethx.autoconf=1"
    sysctl -w "net.ipv6.conf.$ethx.accept_ra=1"
    ;;
esac

# waitslaac
# haveipv6The address is skipped, no matter it isslaacordhcpv6
# Because it will be theretranscheck within
# Wait here5seconds is enough because before trying to getdhcp6It also took a while
for i in $(seq 5 -1 0); do
    is_have_ipv6 && break
    echo "waiting slaac for ${i}s"
    sleep 1
done

# Record whether there is a dynamic address
# Since I haven’t set up static yetip，So there is an entry indicating that there is a dynamic address
is_have_ipv4_addr && dhcpv4=true || dhcpv4=false
is_have_ipv6_addr && dhcpv6_or_slaac=true || dhcpv6_or_slaac=false
is_have_ipv6_gateway && ra_has_gateway=true || ra_has_gateway=false

# If automatically obtained IP If it is not the one before reinstallation, change it to static and use the previous one. IP
# Compare only IP，Don’t compare masks/gateway because
# 1. Assumption mask/The gateway makes it impossible to access the Internet. It will be detected later and changed to static.
# 2. openSUSE wicked dhcpv6 yes 64 bit mask，aws lightsail The same is true for the template, and other dhcpv6 Software is 128 bit mask
if $dhcpv4 && [ -n "$ipv4_addr" ] && [ -n "$ipv4_gateway" ] &&
    ! [ "$(echo "$ipv4_addr" | cut -d/ -f1)" = "$(get_first_ipv4_addr | cut -d/ -f1)" ]; then
    echo "IPv4 address obtained from DHCP is different from old system."
    should_disable_dhcpv4=true
    flush_ipv4_config
fi
if $dhcpv6_or_slaac && [ -n "$ipv6_addr" ] && [ -n "$ipv6_gateway" ] &&
    ! [ "$(echo "$ipv6_addr" | cut -d/ -f1)" = "$(get_first_ipv6_addr | cut -d/ -f1)" ]; then
    echo "IPv6 address obtained from SLAAC/DHCPv6 is different from old system."
    should_disable_accept_ra=true
    should_disable_autoconf=true
    flush_ipv6_config
fi

add_missing_ipv4_config
add_missing_ipv6_config

# examine ipv4/ipv6 Whether connected to the Internet
ipv4_has_internet=false
ipv6_has_internet=false
test_internet

# If you cannot access the Internet and automatically obtain the mask/If the gateway is not the one before reinstallation, change it to static
# ip_addr include IP/mask, so it can be used to determine whether the masks are different
# IP Different situations have been changed to static before.
if ! $ipv4_has_internet &&
    $dhcpv4 && [ -n "$ipv4_addr" ] && [ -n "$ipv4_gateway" ] &&
    ! { [ "$ipv4_addr" = "$(get_first_ipv4_addr)" ] && [ "$ipv4_gateway" = "$(get_first_ipv4_gateway)" ]; }; then
    echo "IPv4 netmask/gateway obtained from DHCP is different from old system."
    should_disable_dhcpv4=true
    flush_ipv4_config
    add_missing_ipv4_config
    test_internet
fi
# It may be static IPv6 But can RA Get the gateway, so add || $ra_has_gateway
if ! $ipv6_has_internet &&
    { $dhcpv6_or_slaac || $ra_has_gateway; } &&
    [ -n "$ipv6_addr" ] && [ -n "$ipv6_gateway" ] &&
    ! { [ "$ipv6_addr" = "$(get_first_ipv6_addr)" ] && [ "$ipv6_gateway" = "$(get_first_ipv6_gateway)" ]; }; then
    echo "IPv6 netmask/gateway obtained from SLAAC/DHCPv6 is different from old system."
    should_disable_accept_ra=true
    should_disable_autoconf=true
    flush_ipv6_config
    add_missing_ipv6_config
    test_internet
fi

# To delete the non-networking protocolip，because
# 1 Oracle Cloud Management Panel Addedipv6address then cancel
#   will still be allocatedipv6address, butipv6No internet
#   at this timealpineOnly useipv6downloadapk，Instead of knowingipv4download
# 2 haveipv4address but noipv4Gateway situation(vultr $2.5 ipv6 only)，aria2Will useipv4download

# hypothesis ipv4 ipv6 on different network cards，ipv4 Can access the Internet but ipv6 If you cannot access the Internet, you must delete it at this time. ipv6
# The ____ does not work ipv4_has_internet && ! ipv6_has_internet Judgment, because it judges the same network card
if ! $ipv4_has_internet; then
    if $dhcpv4; then
        should_disable_dhcpv4=true
    fi
    flush_ipv4_config
fi
if ! $ipv6_has_internet; then
    # prevent deletion of IPv6 Pass again after SLAAC get
    # No need to judge || $ra_has_gateway ，Because there is no IPv6 Address but there is IPv6 There will be no download problems when using the gateway
    if $dhcpv6_or_slaac; then
        should_disable_accept_ra=true
        should_disable_autoconf=true
    fi
    flush_ipv6_config
fi

# If you are connected to the Internet but do not get the default DNS，then add our DNS

# There is a situation where there are multiple network cards, and the network card that can access the Internet completes the script first, and the network card that cannot access the Internet completes it later.
# The network card that cannot access the Internet passes through flush_ipv4_config Delete those that cannot access the Internet IP and dns
# （The original plan was to delete the network card that cannot access the Internet. dhcp4 obtained dns，But in fact it is impossible to distinguish）
# So add it directly here dns，Does not determine whether the network is connected
if ! is_have_ipv4_dns; then
    echo "nameserver $ipv4_dns1" >>/etc/resolv.conf
    echo "nameserver $ipv4_dns2" >>/etc/resolv.conf
fi
if ! is_have_ipv6_dns; then
    echo "nameserver $ipv6_dns1" >>/etc/resolv.conf
    echo "nameserver $ipv6_dns2" >>/etc/resolv.conf
fi

# Pass parameters to trans.start
netconf="/dev/netconf/$ethx"
mkdir -p "$netconf"
$dhcpv4 && echo 1 >"$netconf/dhcpv4" || echo 0 >"$netconf/dhcpv4"
$dhcpv6_or_slaac && echo 1 >"$netconf/dhcpv6_or_slaac" || echo 0 >"$netconf/dhcpv6_or_slaac"
$should_disable_dhcpv4 && echo 1 >"$netconf/should_disable_dhcpv4" || echo 0 >"$netconf/should_disable_dhcpv4"
$should_disable_accept_ra && echo 1 >"$netconf/should_disable_accept_ra" || echo 0 >"$netconf/should_disable_accept_ra"
$should_disable_autoconf && echo 1 >"$netconf/should_disable_autoconf" || echo 0 >"$netconf/should_disable_autoconf"
echo "$ethx" >"$netconf/ethx"
echo "$mac_addr" >"$netconf/mac_addr"
echo "$ipv4_addr" >"$netconf/ipv4_addr"
echo "$ipv4_gateway" >"$netconf/ipv4_gateway"
echo "$ipv6_addr" >"$netconf/ipv6_addr"
echo "$ipv6_gateway" >"$netconf/ipv6_gateway"
echo "$ipv6_extra_addrs" >"$netconf/ipv6_extra_addrs"
$ipv4_has_internet && echo 1 >"$netconf/ipv4_has_internet" || echo 0 >"$netconf/ipv4_has_internet"
$ipv6_has_internet && echo 1 >"$netconf/ipv6_has_internet" || echo 0 >"$netconf/ipv6_has_internet"
