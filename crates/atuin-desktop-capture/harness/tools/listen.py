"""Listens for window activations and ignores them, for as long as it runs.

The bus launcher of at-spi2-core 2.61 and later turns the session's IsEnabled flag on when any
client is listening for events, and Qt applications and Firefox join the accessibility bus only
when the flag is on. Capture is such a client too, so this script lets those applications join
before capture starts. On 2.60 and earlier, the flag follows only the toolkit-accessibility
setting."""
import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi

listener = Atspi.EventListener.new(lambda event: None)
listener.register("window:activate")
Atspi.event_main()
