from dataclasses import dataclass
from enum import Enum


class SupercolliderNodeKind(str, Enum):
    GROUP = "Group"
    SYNTH = "Synth"


@dataclass(slots=True)
class SupercolliderNodeSummary:
    id: int
    name: str | None
    kind: SupercolliderNodeKind


@dataclass(slots=True)
class SupercolliderServerState:
    sample_rate: float | None
    num_output_channels: int | None
    nodes: list[SupercolliderNodeSummary]

    @staticmethod
    def bootstrap_placeholder() -> "SupercolliderServerState":
        return SupercolliderServerState(
            sample_rate=None,
            num_output_channels=None,
            nodes=[
                SupercolliderNodeSummary(
                    id=0,
                    name="root",
                    kind=SupercolliderNodeKind.GROUP,
                ),
                SupercolliderNodeSummary(
                    id=1000,
                    name="example_synth",
                    kind=SupercolliderNodeKind.SYNTH,
                ),
            ],
        )
