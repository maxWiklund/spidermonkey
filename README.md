# spidermonkey



## Config
`config.yaml`
```yaml

scan_settings:
  rescan_interval: "4m"       # Time until rescanning (e.g., "10m", "1h", "30s")
  pre_scan_commands:
    - echo "Pull latest changes from git"
    - git pull
  scan_directory: "~/dev/firefox"
  exclude_patterns:
    - ".git/"
  endpoint: "127.0.0.1:3000"
```

```shell
spidermonkey -c config.yaml
```