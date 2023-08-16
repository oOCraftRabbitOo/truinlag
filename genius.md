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
- the manager task (which is perpetually blocked as it is waiting for connections).
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
