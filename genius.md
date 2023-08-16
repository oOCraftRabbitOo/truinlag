# How to solve the problem with multithreading:

## Basics

- There is one truinlag engine with which every client interfaces
- Every client establishes a two-way connection to the engine
- The engine is completely inactive unless a client makes a request
- Upon receiving a request, the engine will always return a response and may optionally change internal data or send a command to every client
- The connections need to be two-way, so there's a lot to keep track of. It is done as follows:  

## Threads

- There is one main thread which is the only one that actually changes data
- There is one thread per client that is connected to the engine
- The client threads are to relay data from the clients to the main thread and vice versa
- Each client thread is connected to the main thread through channels and to the client through ipc

## Channels


# How to solve the problem ding

## Problem

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

## Brainstorming a solution


