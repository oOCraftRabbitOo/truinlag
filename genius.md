# The i/o task issue

It's a long saga, this is a record of all my suffering:

## How to solve the problem with multithreading:

### Basics

- There is one truinlag engine with which every client interfaces
- Every client establishes a two-way connection to the engine
- The engine is completely inactive unless a client makes a request
- Upon receiving a request, the engine will always return a response and may optionally change internal data or send a command to every client
- The connections need to be two-way, so there's a lot to keep track of. It is done as follows:  

### Threads

- There is one main thread which is the only one that actually changes data
- There is one thread per client that is connected to the engine
- The client threads are to relay data from the clients to the main thread and vice versa
- Each client thread is connected to the main thread through channels and to the client through ipc

### Channels


## How to solve the problem ding

### Problem

Okay okay so
I wanna have one central task which is the only one that changes the database or the game state.
Multiple clients should be able to connect to that 'engine' and each connection must be a two-way connection so that the server and the client can both send messages.
In order to facilitate that, there will be a central task which represents the engine and one task per client.
The i/o tasks should be able to receive messages from the clients and relay them to the engine while also listening to the engine and relaying its messages to the client.
The engine is completely passive if no commands are issued, but if one client issues a command the engine may send a response to all clients as well as only to the one that asked.
For the messages coming from the clients to the engine, there will be an mpsc channel where every i/o task has a sender handle.
For the messages from the engine to all clients, there will be a broadcast channel with every i/o task having a receiving handle.
For the messages from the engine to a specific client, the broadcast channel will be used but only the relevant i/o channel will act on the message. if this becomes a problem and a massive waste of memory, i can fix it later lol.
The i/o tasks will need to be waiting for both engine and client messages simultaneously and repeatedly and they need to finish cleanly if something happens which is a problem.
In the i/o taks, there could be two async routines, one listening to the engine in an infinite loop and one listening to the client.
The problem is that the engine routine needs access to the stream that connects to the client in order to send it stuff and the client routine needs access to it too in order to listen.
The stream cannot be managed sensibly by something like a mutex, as the listening operation on the stream occupies it permanently (stream.recv() blocks until something is received).

### The Solution

After some long thinking and some thourough research, i have come up with an incredibly sophisticated solution:
You can clone the stream using [try_clone()](https://doc.rust-lang.org/stable/std/os/unix/net/struct.UnixStream.html#method.try_clone) lol
(i wasted a lot of time)

# Async Structure

## How will I do the task layout?

### initial brainstorming

As established, there's gonna be an engine task which accepts commands and generates responses.
It will have the receiving handle of an mpsc channel and the sending handle of a broadcast channel. (all tokio, obv)
There will be one i/o task per client, each receiving a clone of the mpsc sender handle and the broadcast receiver handle.
In the center, there must therefore be a manager task.
It will be responsible for starting the engine task, creating the channels and giving the engine the unique handles to those channels.
Afterwards, it will accept new connections and create i/o tasks for those connections and it will equip those with the streams and with the channel handles they need.
So this manager task, because it will be waiting for new connections, will be blocked most of the time.
How do I shut it down?

### How do i shut it down?

There needs to be some sort of mechanism to shut down
- The engine task (which is perpetually blocked as it is waiting for signals),
- The i/o tasks (which are perpetually blocked as they are waiting for signals),
- the manager task (which is perpetually blocked as it is waiting for connections). (funny line)
This might be a problem.

There are gonna be multiple ways for a shutdown to be triggered:
- A shutdown command coming from a client,
- A shutdown command coming from the system (C-c),
- A panic somewhere. (I think i'll just ignore this one for now)

I think it would be the most sensible if the engine triggered shutdowns as everything communicates with it anyways.
So, first, how would a shutdown be initiated?
If it's initiated from
- A client, the corresponding i/o task will send a shutdown command to the engine through the mpsc channel
- The system (C-c), the (presumably existing) task monitoring those signals will just send the same command through the mpsc channel.

Once a shutdown is initiated, the engine will shut down
- The i/o tasks by sending a shutdown signal through the broadcast channel, the i/o tasks will probably be using `select!` and be able to be shut down through an engine signal or a client disconnect
- the manager task through a oneshot channel, the manager task will therefore probably be using `select!` too (it won't be shut down, it will just stop accepting new connections and enter a shutdown phase)
- itself using the highly sophisticated technology called the `break` keyword
The manager task will probably then `join!` all the tasks but with a time limit using `select!`, so that it can give an error signal if the tasks don't finish properly.

## A problem emerged while coding

### The problem

As described previously, in order to communicate with the i/o tasks, the engine will hold the sender handle to a broadcast channel.
Well, I just found out that one cannot simply clone the receiver handle of a broadcast channel, new ones are generated using `tx.subscribe()`.
The problem now is, that the manager needs to give every new i/o task a receiving handle to the broadcast channel and that's tough if the thing generating those handles is in the engine's possession

### The solution

The easiest way to get handles from the engine to the i/o tasks is by having it send them to the manager and it passing them to the i/o tasks as they are created.
For that to work, the manager first needs to ask for another broadcast handle, then the engine neeeds to react by sending such a handle to the to the manager.
For _that_ to work, the manager needs a way to send signals to the engine and the engine needs a way to send signals to the manager.
The manager can already send messages to the engine using the normal mpsc channel.
The engine also has a way to send sth to the manager, but it's through the oneshot channel that's supposed to shut down the programme.
One could now simply change that oneshot channel to a mpsc channel that is permanent, but i don't think that's a good idea, as the whole shutdown process revolves around the manager awaiting new connections and engine shutdown messages simulatneously using `select!`.
So that oneshot channel should be reserved for that shutdown message only.
What could be done is that there could be *another* permanent mpsc channel, only going from the engine to the manager.
I think it would be smarter though to have the manager create a oneshot channel whenever he needs a new handle and to send the sender of that channel to the engine.

# About multiple sessions

We ideally want multiple sessions, so that trainlag can run for two groups of people at the same time.
Some information, saved in trainlag, should be accessible only to the current session and some should be accessible to everyone.
Here's a short list of that:
Global:
- Challenges
- Players
- Config
Local:
- Teams
It makes sense to not allow modification of any data (so challenges, players, teams, config) while a session is running.
some checks that need to be present when starting a game:
- One player can only play in one session -> A player must be marked as unavaliable while playing and when starting a session, it must be made sure that all participants are avaliable
- No two players, challenges or teams should have the same name. Players have nicknames, they should all be unique too.

Another thing:
How do i handle editing data while a game is running?
I think the smartest way to go about it is to have the engine load the data from the database into memory before starting a game.
Then, changes made to data regarding the current session just won't apply immediately.
In addition, the engine can disallow changing data through the current session.
so, even though player names cannot be changed while the game is running, a different session could change those names.
It just wouldn't apply to the session as the game is still running.

(also i think i will handle game modes with sessions.)

Game modes are gonna be hard coded, but sessions shouldn't be.
I will probably have to save sessions in the database too.
Sessions will have fields like gamemode, discordserver, name, etc. while gamemode is an enum.

## more shit

How does the communication work now?
Every engine command should get a response, as well as possibly a global broadcast.
So when an io task / a client sends a command, they will wait for a direct response.
There should therefore probably be two kinds of responses (-> enum) with the same actions (-> struct {kind, action}?):
A response and a broadcast.
So clients and io tasks would destinguish between responses and broadcasts.
On the client's side, the response should be the return type of the send() function and broadcasts should be received through recv().
There are two problems with this approach:
First, the i/o task receives messages through its part that has the socket listener.
That part will send the send handle of a oneshot channel to the engine through which it will send the response.
The problem is that this part of the i/o task cannot send any messages to the client as it doesn't have the socket's sending half.
The response (and therefore the broadcast receive handle) must somehow get to the i/o task's half which has the sending socket.
I cannot simply create a channel between the two as the socket sender task is already listening for signals from the engine.
No task should be listening to two things at once.
I could just use the broadcast channel instead of the oneshot channel, but that would come with a lot of extra overhead.
I got it
I can create two intermediate tasks which are connected to the sender socket half of the i/o task through an mpsc channel
The first of these tasks listens to the broadcast channel and forwards the commands to the mpsc channel.
The second listens for oneshot receivers from the other half and then listens on that receiver and relays the resulting message over the mpsc channel.
Okay, second problem:
I forgot what the second problem is. It was something on the client's side, i'm sure.
I'm just gonna go through how this could work on the client side.
On a connection, there are the send() and recv() methods.
As it stands, these are standalone methods, basically just masking a unix stream.
In order to fulfill my tall orders, i'll need those methods to interact with another task which is created as the connection is established. 
There will be a background task listening for messages from the engine.
It will have two mpsc channels, one going from the helper task to the sender half / send() and one going to the receiver half / recv().
This helper task will distinguish between responses and broadcasts and relay them to the appropriate receivers.
The send method will simply send a command directly through the socket, just as it is doing already, and will then wait on its channel.
The only problem might be a graceful shutdown.
What makes that a challenge is that, in most practical implementations of the api, the connection will be split in two.
If the engine shuts down, an error has to be sent to both halves of the connection from the helper task.
And if one of the halves is dropped, how does the other know?
Well, it doesn't and it doesn't need to.
It just has to be made sure that one half being dropped doesn't cause a panic or something.
lol

# How the fuck do i do the api?

Okay, i need some tasks and channels.
I can have a send request mpsc channel and a send manager task, which will send commands to the engine.
Then i can have a receiver task, which receives engine messages and passes them on to the distributor task to an mpsc channel.
There will be a distributor task which listens on an mpsc channel.
That channel will be used by the receiver task to send received messages there.
It will also be used by the send request task in order to inform it of responses.
A response info message will contain a oneshot channel and an ID.
The distributor task can send a response back to the calling function using that info.
It will pass broadcasts through to an mpsc channel, which sends them to the appropriate method.
Well, that was easy.

Actually, no, not easy.
One should be able to build a client without a send handle as well as one without a broadcast receiver.
The tasks running in the background should not shut down if only the send handle or only the receiver are dropped.
They should wait until both are dropped.
How do i even detect whether the receive handle is down?
I'll solve that later.

Alright so there is a send manager and a broadcast manager and all the broadcast manager does is relay a message unnecessarily.
I can await those two using a vector in another task which i await with the others using select!.
If both the broadcast and send managers shut down, i shut down all tasks and print something appropriate.
If there's a disconnect aka truinlag crashes, i shut down and print something appropriate too.

*can i push to this repo?*
*hmmmm*
*jetzt müessts gah*
