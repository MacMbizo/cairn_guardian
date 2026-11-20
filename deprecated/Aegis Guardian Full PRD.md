
# Aegis Guardian
## Startup-Grade Product Requirements Document (PRD)

Version: 1.0
Status: Vision / Pre-Series A Specification

# Executive Summary
Aegis Guardian is an AI-native desktop platform that discovers, understands, organises, secures, archives, and maintains an entire personal or professional computing environment. Unlike traditional file managers, cleanup tools, search tools, or knowledge management systems, Aegis Guardian creates a semantic model of a user's digital life and continuously improves the structure of that environment.

# Vision
Create a self-maintaining intelligent file system that understands meaning rather than location.

# Problem Statement
Users accumulate years of documents, source code, media, downloads, archives, notes, screenshots, datasets, backups and duplicates. Conventional folders fail at scale.

# Product Goals
- Create a semantic digital twin of the computer
- Automatically classify and organise content
- Detect duplicate, orphaned and stale assets
- Provide natural-language retrieval
- Maintain complete auditability and rollback
- Protect sensitive information
- Operate primarily on-device

# Personas
## Researcher
Needs literature management and knowledge discovery.

## Developer
Needs repository, dependency and project intelligence.

## Trader
Needs statement, report and strategy organisation.

## Knowledge Worker
Needs automated classification and retrieval.

# Product Principles
1. Understand before modifying.
2. Always explain decisions.
3. Maintain reversibility.
4. Keep humans in control.
5. Privacy by design.

# Functional Requirements

## FR-001 Global Discovery Engine
System scans:
- Internal drives
- External drives
- NAS storage
- OneDrive
- Google Drive
- Dropbox
- Email exports
- Browser download locations

Collected metadata:
- Path
- Hash
- MIME type
- Owner
- Created date
- Modified date
- Access frequency

## FR-002 Content Intelligence Layer
Supported formats:
- PDF
- DOCX
- XLSX
- PPTX
- TXT
- Markdown
- Images
- Audio transcripts
- Video transcripts
- Source code

Capabilities:
- OCR
- Entity extraction
- Topic detection
- Sentiment classification
- Project identification

## FR-003 Knowledge Graph
Entities:
- Projects
- Documents
- People
- Companies
- Topics
- Repositories
- Accounts

Relationships:
- references
- belongs_to
- derived_from
- duplicate_of
- related_to

## FR-004 Duplicate Intelligence
Levels:
1. Exact duplicate
2. Near duplicate
3. Semantic duplicate
4. Version-chain detection

## FR-005 Autonomous Refiling
Stages:
- Analyse
- Simulate
- Approve
- Execute
- Audit
- Rollback

## FR-006 Workspace Health
Scores:
- Organisation
- Storage
- Security
- Duplication
- Searchability

## FR-007 AI Search
Examples:
- Show latest dissertation draft
- Find all files related to portfolio optimisation
- Find screenshots from Aegis risk engine

## FR-008 Security Intelligence
Detect:
- API keys
- Passwords
- Private certificates
- Identity documents
- Financial data

## FR-009 Archive System
Lifecycle:
Active -> Warm -> Cold -> Archive

## FR-010 Continuous Monitoring
Watch:
- File creation
- File deletion
- File move
- Project changes

# System Architecture

## Desktop Application
Technology:
- Tauri
- React
- TypeScript
- Tailwind

## Core Engine
Technology:
- Python
- Rust services
- gRPC

Modules:
- Discovery Engine
- Classification Engine
- Knowledge Graph Engine
- Automation Engine
- Security Engine

## AI Layer
Models:
- Phi
- Llama
- Mistral
- Cloud model optional

Functions:
- Summarisation
- Classification
- Embeddings
- Recommendations

## Vector Database
Options:
- Qdrant
- Chroma
- Weaviate

Embedding storage:
- Chunk embeddings
- Project embeddings
- Entity embeddings

## Relational Database
PostgreSQL

Tables:
- files
- folders
- projects
- entities
- relationships
- actions
- audits

# Data Model

## File Object
Attributes:
- id
- path
- title
- checksum
- size
- classification
- confidence
- project

## Project Object
Attributes:
- name
- description
- members
- repositories
- assets

# AI Agent Framework

## Librarian Agent
Classifies knowledge assets.

## Curator Agent
Designs folder structures.

## Housekeeper Agent
Removes clutter.

## Archivist Agent
Manages lifecycle.

## Security Agent
Protects sensitive data.

## Developer Agent
Understands repositories and environments.

# Decision Framework
Every action must store:
- Reason
- Confidence
- Evidence
- Timestamp
- Recovery path

# User Interface

## Dashboard
Widgets:
- Health score
- Duplicate score
- Storage score
- Security alerts

## Digital Twin Viewer
Interactive graph showing relationships between files, people and projects.

## Reorganisation Simulator
Preview all changes before execution.

# Security Requirements
- Local-first processing
- AES-256 encrypted metadata
- Role-based access
- Audit trails
- Secure deletion

# Non-Functional Requirements

Performance:
- 1 million files supported
- Incremental indexing
- Background processing

Availability:
- Recover after restart
- Fault-tolerant jobs

Scalability:
- Multi-drive support
- Distributed indexing roadmap

# API Design

REST API
/graphql endpoint
Local agent protocol
Plugin SDK

# Plugin Marketplace
Examples:
- Trading plugin
- Research plugin
- Developer plugin
- Media plugin
- Legal archive plugin

# Research Mode
Features:
- Paper ingestion
- Citation extraction
- Knowledge mapping
- Literature clustering
- Research gap discovery

# Developer Mode
Features:
- Repo analysis
- Dependency mapping
- Dead project detection
- Secret scanning
- Environment cleanup

# Trading Mode
Features:
- Statement grouping
- Strategy organisation
- Broker record management
- Tax record classification

# Metrics
North Star Metric:
Percentage of digital assets correctly understood.

Supporting Metrics:
- Duplicate reduction
- Search success rate
- User time saved
- Storage reclaimed

# MVP Scope
- Discovery engine
- Knowledge graph
- Duplicate detection
- AI classification
- Search
- Refiling simulator

# V2
- Autonomous actions
- Agent framework
- Security intelligence
- Multi-device synchronisation

# V3
- Digital twin
- Predictive housekeeping
- Personal knowledge operating system

# Monetisation
Free:
- Local indexing
- Search

Pro:
- AI agents
- Automation
- Graph analytics

Enterprise:
- Team knowledge graph
- Governance
- Compliance

# Risks
- Misclassification
- Privacy concerns
- Excessive automation
- Performance overhead

Mitigations:
- Human approval
- Rollback
- Local processing
- Confidence thresholds

# Long-Term Vision
Aegis Guardian becomes an intelligent operating layer that understands every document, project, relationship and workflow across a user's entire digital environment.
