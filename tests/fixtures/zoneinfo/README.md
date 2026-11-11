# TZif regression fixtures

`UTC` and `New_York` are compiled IANA timezone data copied from Debian
`tzdata 2026b-0+deb13u1` (`Etc/UTC` and `America/New_York`). IANA timezone data
is public domain. They exercise actual 64-bit TZif blocks and transition tables;
tests never read the executing host's timezone database.
