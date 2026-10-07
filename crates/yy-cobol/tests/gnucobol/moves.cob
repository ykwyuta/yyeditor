      * CBL.MOVE（yy-cobol の move_elementary・move_group）と比べる MOVE。
      * 受け取りの項目を順に並べたレコードを 1 件書く（examples/gnucobol_moves.rs が同じ MOVE を
      * yy-cobol で行ったバイト列と比べる）。
       IDENTIFICATION DIVISION.
       PROGRAM-ID. MOVES.
       ENVIRONMENT DIVISION.
       INPUT-OUTPUT SECTION.
       FILE-CONTROL.
           SELECT OUT-FILE ASSIGN TO "moves.dat"
               ORGANIZATION IS SEQUENTIAL.
       DATA DIVISION.
       FILE SECTION.
       FD  OUT-FILE.
       01  OUT-REC.
           05  O1    PIC 9(3).
           05  O2    PIC 9(3).
           05  O3    PIC X(8).
           05  O4    PIC X(8).
           05  O5    PIC X(4).
           05  O6    PIC X(12).
           05  O7    PIC S9(5)V99.
           05  O8    PIC S9(5)V99.
           05  O9    PIC X(5) JUSTIFIED RIGHT.
           05  O10   PIC X(10).
           05  O11.
               10  O11-ID  PIC 9(3).
               10  O11-NM  PIC X(4).
           05  O12   PIC S9(4) COMP.
           05  O13   PIC S9(3)V99 COMP-3.
           05  O14   PIC ZZ,ZZ9.99-.
           05  O15   PIC 9(5) COMP-3.
           05  O16.
               10  O16-A   PIC 9(3).
               10  O16-B   PIC S9(3) COMP-3.
       WORKING-STORAGE SECTION.
       01  N52   PIC S9(5)V99.
       01  N5    PIC 9(5).
       01  S3    PIC S9(3).
       01  N9    PIC 9(9).
       01  X5    PIC X(5).
       01  ED    PIC ZZ,ZZ9.99-.
       01  SRC.
           05  S-ID   PIC 9(3).
           05  S-NM   PIC X(4).
           05  S-AMT  PIC S9(3)V99 COMP-3.
       01  WIDE  PIC X(10).
       PROCEDURE DIVISION.
           OPEN OUTPUT OUT-FILE.
           MOVE 12345.67 TO N52.  MOVE N52 TO O1.
           MOVE -7.5 TO N52.  MOVE N52 TO O2.
           MOVE 42 TO N5.  MOVE N5 TO O3.
           MOVE -42 TO S3.  MOVE S3 TO O4.
           MOVE 123456789 TO N9.  MOVE N9 TO O5.
           MOVE -1234.5 TO ED.  MOVE ED TO O6.
           MOVE ED TO O7.
           MOVE "00123" TO X5.  MOVE X5 TO O8.
           MOVE "AB" TO O9.
           MOVE 7 TO S-ID.  MOVE "AB" TO S-NM.  MOVE -1.5 TO S-AMT.
           MOVE SRC TO O10.
           MOVE "12345XY" TO WIDE.  MOVE WIDE TO O11.
           MOVE -12345.67 TO N52.  MOVE N52 TO O12.
           MOVE 9876.543 TO N52.  MOVE N52 TO O13.
           MOVE -0.5 TO N52.  MOVE N52 TO O14.
           MOVE N9 TO O15.
           MOVE SRC TO O16.
           WRITE OUT-REC.
           CLOSE OUT-FILE.
           STOP RUN.
