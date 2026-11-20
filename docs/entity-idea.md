# The entity idea

Written 2026-11-20. This is the idea only: what an entity is and what follows from it. How
it is stored and built is left open on purpose. `ui-design-plan.md` holds the editor and the
aliveness model this fits into.

## The idea

**What you place in a scene is a thing that lives there, not a recording.** A blackbird, a
frog, a stream, a bell. You name the bird. The bird has a voice, and the recordings are what
it has to say. It decides which phrase to sing and when.

Today it is the other way round. A "Frog" is one sound file with a label on it, looping
without a pause. The scene has an entity around it (an id, a name, a place), but the thing
it plays is still a file. So the ear hears a loop, the editor shows a file, and there is
nothing to hang behaviour on.

Turning it round changes what the source of a sound is:

| | A file is the source (today) | An entity is the source |
|---|---|---|
| You add | `frog.mp3` | A frog |
| It sounds | The same recording, end to end, again | One of its croaks, then a pause, then another |
| It is silent | Never | Most of the time, as a real one is |
| Three of them | The same loop three times, in step | Three frogs, each croaking in its own time |
| It rains | Nothing changes | It can fall silent, or start up |
| The editor shows | A file name and a level | What it is, what it can say, what it is doing now |

## Kind and individual

Two levels, and the difference matters.

- **A kind** is what all blackbirds have in common: the voice, how loud it is, how the sound
  leaves it, how it sings, when it is active, how it gets about. A kind is made once and
  kept in the library.
- **An individual** is one blackbird in this place: its own name, where it lives, and
  whatever is different about it. "The blackbird in the oak."

An individual takes everything from its kind and changes only what differs. Three
blackbirds are three individuals of one kind. They share a voice and a way of singing, and
still never sing the same phrase at the same moment, because each makes its own choices.

A kind is not only for creatures. A stream, a church bell, a campfire and a narrator are
kinds too.

## What an entity has

The same five questions the inspector already asks, read from the entity's side.

- **A voice: what can it say?** A set of takes. A take is one thing it can utter from start
  to end: a phrase of song, one croak, one stroke of the bell. Together they are its
  repertoire. A voice can also be made fresh instead of recorded (wind, rain, a river), and
  then it has no takes, because it never says the same thing twice anyway.
- **A shape: how does the sound leave it?** In all directions, in a beam, or filling the
  air. A bird sings forward. A stream is wide. See "Shape and reach" below.
- **A place: where does it live?** A spot, a height, and later a territory or a set of
  perches.
- **A rhythm: when does it sound?** All the time, now and then, or in bouts, and under
  which conditions: at dawn, not in the rain.
- **A way of moving: does it travel?** Stays put, circles, wanders its territory, hops
  between perches.

The simplest entity has one take, sounds all the time and stands still. That is exactly
what every sound is today, so nothing that exists stops being possible. A drum loop or a
piece of music is an entity of that plain sort.

## Shape and reach

Added 2026-11-20. Two things an entity should carry that it does not carry today.

**The width of a beam is a value, not a choice of three.** Today a sound is Omni, Focused or
Narrow, or Ambient when its generator says so. Those are four points on what is really one
range, and most things fall between them. An entity has:

- **A width**, from a tight beam through a half circle to all round. Omni is the widest end
  of the same range, not a separate kind of thing.
- **A facing**, which only matters once the width is less than all round.
- **How much is heard behind it.** A beam is rarely silent at the back. A bird heard from
  behind is duller and softer, not gone.
- **A size**, from a point to wide. A cricket is a point. A stream is ten metres long.
  Ambient is the far end: so large that it has no direction left.

The named shapes stay, as starting points you pick and then adjust, the way a preset does.

**Reach is how far it carries.** A frog is heard across the pond, a cricket only from
nearby, thunder for kilometres. Reach comes from how loud the thing is at its source and
how its sound travels, and it is what you see on the map as the area where the entity can
still be heard. It belongs to the entity because it is the first thing you want to know
when you place one: from where will I hear this?

**Every kind starts with a shape and a reach that fit it.** Nobody should have to know that
a frog croaks at about 65 dB or that a drum radiates mostly upward and forward. The kind
knows. Dropped into a scene, each one already sounds like itself:

| Kind | Shape | Size | Reach |
|---|---|---|---|
| Frog | All round | A point | Across the pond |
| Blackbird | A wide beam, forward and up | A point | A garden or two |
| Cricket | All round | A point | A few metres |
| Drum | A broad beam off the skin | Small | The whole space |
| Stream | All round | Long and wide | Fades within some tens of metres |
| Wind | No direction | Everywhere | Everywhere |
| Bell | All round | Small | Very far |

The values in this table are examples of the idea, not measured numbers.

An individual then changes only what is different about it: this blackbird faces the house,
that frog is a loud one. Anything left alone stays as the kind has it, and a value that was
changed can be put back.

## What an entity does

An entity is always in a state, and the state is what makes it feel alive.

- **It is sounding or it is silent.** Between two phrases there is a gap, and the gap is
  never the same length twice.
- **It chooses.** Each time it sounds it picks a take. Either varied, never the same one
  twice running, or in bouts: the same take a few times over and then a switch, the way a
  great tit holds on to one song for a while.
- **It is never exactly the same.** Each take sounds a little louder or softer, a little
  higher or lower than the last time.
- **It has a cause.** It sings more at dawn and stops when the rain starts. A change you
  hear has a reason you could point at.
- **It can be asked.** At any moment the entity can say what it is doing: singing its third
  phrase, or resting with six seconds to go.

## What you see

Because the entity is the source, every part of the editor shows the entity and not a file.

- **The library lists kinds.** "Blackbird, 3 phrases", not three file names. Pressing one
  lets you hear the kind as it would sound in a place: a phrase, a pause, another phrase.
- **The layers list individuals.** "Blackbird in the oak", with a small sign of whether it
  is sounding right now.
- **The inspector shows the voice as a list of takes.** Every take by name, the one
  sounding now lit, and "Silent" when it is between phrases. Each take has a button to hear
  it on its own. This is the panel the spatial project had, and it is the proof on screen
  that the repertoire works.
- **The map shows life.** A marker lights when its entity sounds and rests between. A
  meadow with five birds flickers the way it sounds.
- **The activity strip shows each phrase as a mark**, so the rhythm of a place can be read
  at a glance.

## What follows from it

- **Silence costs nothing.** An entity that is resting plays nothing, so it needs none of
  the engine's 16 voices while it rests. A place could then hold more creatures than
  voices, as long as they do not all sound at once.
- **Behaviour has one home.** Takes, gaps, movement and reactions to the weather all belong
  to the same named thing. Without the entity each of those would need its own place to
  live.
- **A kind can be shared.** A good blackbird, with its takes and its habits, drops into any
  scene and already behaves like one.
- **The place can be described in words.** "Two blackbirds, a robin and a stream" is a
  scene. A list of files is not.
- **Entities can know about each other.** A robin answers a blackbird. The frogs stop when
  something comes near. None of that can be said about files.

## What it is not

- **Not a voice.** A voice is an engine slot. An entity keeps its identity while the engine
  hands it a slot and takes it away again.
- **Not a file.** A file is one take in somebody's repertoire.
- **Not a simulation of a bird.** It has just enough behaviour to never repeat and to react
  to the place. It does not eat, sleep or nest.

## Open questions

- **May an individual have takes of its own?** A blackbird with a signature phrase the
  others do not have, or only a choice from its kind's takes.
- **Is a chorus one entity or many?** Ten frogs as ten individuals, or one "frog pond" that
  croaks from many spots. The first is truer, the second cheaper.
- **Does a voice have more than one register?** Song, a contact call and an alarm call are
  different things a bird says for different reasons. One repertoire, or several with a
  reason attached to each.
- **Where does it sing from?** One spot, or a few perches it moves between, singing a bout
  from each.
- **How much does it notice?** The weather and the time of day for certain. Other entities
  and the listener are possible and add a lot of life, and a lot of work.
- **What is a generated sound's "take"?** Thunder makes every strike new. It behaves like a
  repertoire with no recordings in it, and should look the same in the editor.
- **What is on disk today.** `assets/birds/` has 20 takes of five garden birds: dunnock 5,
  blue tit 6, robin 5, blackbird 3, great tit 1. One take is not enough for bouts, so the
  great tit needs more. `bird_1.mp3` and `bird_2.mp3` are unnamed.
