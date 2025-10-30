# Kyber Windows Service

## What is Kyber Windows Service

Kyber Windows Service is the component that is registered as a Windows Service and that starts the Controller executable.

The goals of the service are:
* Get the required credentials to get access to the Secure Desktop
* Automatic start at Windows startup
* Plug a Virtual Screen if required

## Configure Kidd integration

[Kidd](https://gitlab.com/kyber-labs/kyber-streamer/kidd) can be used to plug a virtual monitor on an headless machine.

The default behavior of the service is to plug a virtual monitor if there is no monitor detected. However, this behavior can be changed.

```toml
[windows_service.kidd]
# Enable or disable Kidd
#
# Possible values
# * true: Kidd is enabled
# * false: Kidd is disabled
#
# Default: true
#enabled = true

# Define the strategy to apply to plug a screen or not
#
# Possible values:
# * "when_no_monitor": Plug monitors only if no monitor is detected
# * "always": Always plug the configured number of monitors
# * "minimal_monitor_count": Plug only enough monitors to reach monitor_count
#
# Default: "minimal_monitor_count"
#plug_strategy = "minimal_monitor_count"

# Number of virtual monitors to create
#
# Default: 1
#monitor_count = 1

# Add one or more display mode for the plugged monitor.
# The section can be specified multiple times to add multiple modes.
#[[windows_service.kidd.extra_modes]]
#width = 1920
#height = 800
#vsync = 60
```
