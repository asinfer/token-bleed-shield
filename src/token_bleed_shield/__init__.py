"""Token Bleed Shield (TBS)
Preventive workflow to stop bleeding AI API costs before generation starts.
"""

__version__ = "0.1.0"

from .config import Enforcement, ShieldConfig
from .contract import ContractSnapshot, load_contract, parse_contract
from .policy import CircuitBreakerPolicy, Signal

__all__ = [
    "__version__",
    "CircuitBreakerPolicy",
    "ContractSnapshot",
    "Enforcement",
    "ShieldConfig",
    "Signal",
    "load_contract",
    "parse_contract",
]
